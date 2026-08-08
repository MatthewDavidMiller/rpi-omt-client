// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]
#![cfg_attr(feature = "desktop", windows_subsystem = "windows")]

/// Which actions the form as typed allows.
///
/// This is deliberately outside the `desktop` module and free of egui: a
/// disabled button is a validation rule, and restating the core's rules in the
/// view is how the Wi-Fi button came to accept a 64-character passphrase that
/// `validate_wifi` then refused. Compiled without the feature too, so
/// `cargo test -p rpi-omt-deployer` cannot quietly run nothing.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod gates {
    use omt_deployer_core::{Secret, WifiSettings, valid_host, valid_username, validate_wifi};

    /// The connection and deployment fields as the operator has typed them.
    pub struct Form<'a> {
        pub host: &'a str,
        pub user: &'a str,
        pub password: &'a str,
        pub project_root: &'a str,
        pub wifi_ssid: &'a str,
        pub wifi_password: &'a str,
    }

    impl Form<'_> {
        /// Everything `omt_deployer_core::connect` will insist on.
        pub fn can_connect(&self) -> bool {
            valid_host(self.host) && valid_username(self.user) && !self.password.is_empty()
        }

        pub fn can_deploy(&self) -> bool {
            self.can_connect() && !self.project_root.is_empty()
        }

        pub fn can_apply_wifi(&self) -> bool {
            self.can_connect()
                && Secret::new(self.wifi_password.to_owned()).is_ok_and(|password| {
                    validate_wifi(&WifiSettings {
                        ssid: self.wifi_ssid.to_owned(),
                        password,
                        connect: false,
                    })
                    .is_ok()
                })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::Form;

        fn form<'a>(wifi_ssid: &'a str, wifi_password: &'a str) -> Form<'a> {
            Form {
                host: "pi.local",
                user: "root",
                password: "secret",
                project_root: "/src/rpi-omt-client",
                wifi_ssid,
                wifi_password,
            }
        }

        #[test]
        fn connection_fields_gate_every_action() {
            let complete = form("", "");
            assert!(complete.can_connect());
            assert!(complete.can_deploy());
            for incomplete in [
                Form {
                    host: "-pi.local",
                    ..form("", "")
                },
                Form {
                    user: "ro ot",
                    ..form("", "")
                },
                Form {
                    password: "",
                    ..form("", "")
                },
            ] {
                assert!(!incomplete.can_connect());
                assert!(!incomplete.can_deploy());
                assert!(!incomplete.can_apply_wifi());
            }
            assert!(
                !Form {
                    project_root: "",
                    ..form("", "")
                }
                .can_deploy()
            );
        }

        /// The button and the core have to agree, or the operator gets an
        /// enabled control that fails the moment it is used.
        #[test]
        fn the_wifi_button_follows_validate_wifi() {
            assert!(form("studio", "passphrase").can_apply_wifi());
            assert!(form("studio", &"f".repeat(64)).can_apply_wifi());
            // 64 characters that are not a hex PSK: neither a passphrase (8-63)
            // nor a key, which the old length-only check let through.
            assert!(!form("studio", &"z".repeat(64)).can_apply_wifi());
            assert!(!form("studio", "short").can_apply_wifi());
            assert!(!form("studio", &"p".repeat(65)).can_apply_wifi());
            assert!(!form("", "passphrase").can_apply_wifi());
            assert!(!form(&"s".repeat(33), "passphrase").can_apply_wifi());
            assert!(!form("studio", "pass\u{7f}word").can_apply_wifi());
        }
    }
}

/// How the window and its contents answer the display they land on.
///
/// Outside the `desktop` module for the same reason as `gates`: every rule here
/// is only observable on a panel nobody can put in CI -- a 200%-scaled laptop,
/// a 4K desktop, a window dragged to its minimum -- so the arithmetic is kept
/// where it can be tested without one, and the view only calls it. The
/// deployer's old fixed 960x640 window with a 720x480 floor did not fit a
/// 1366x768 panel at 200% scaling at all, and nothing outside a display could
/// have caught that.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod layout {
    /// The window asked for on a display large enough to grant it.
    pub const DEFAULT_SIZE: [f32; 2] = [960.0, 640.0];

    /// The smallest window the views still work in. Every view scrolls, the
    /// navigation wraps, and labels stack above their fields at this size, so
    /// the floor exists only to stop a window collapsing to nothing -- not to
    /// reserve room for a layout. It has to stay small: a 1366x768 panel at
    /// 200% scaling is 683x384 points in total.
    pub const MIN_SIZE: [f32; 2] = [420.0, 320.0];

    /// Share of the monitor an opening window may take. The remainder is for
    /// the furniture no API reports here: task bars, docks, and the title bar
    /// and border drawn outside the inner size this governs.
    const FIT: [f32; 2] = [0.9, 0.85];

    /// Widest a form column is allowed to become. Text fields that follow the
    /// window put a host name in a 3000-point box on a 4K desktop.
    pub const COLUMN_MAX: f32 = 640.0;

    /// Narrowest column that still reads well with labels beside their fields.
    pub const PAIRED_MIN: f32 = 520.0;

    /// Label gutter in a paired row.
    pub const LABEL_WIDTH: f32 = 132.0;

    /// Zoom bounds. Tighter than egui's own 0.2 to 5.0, which reaches
    /// illegible in both directions, and applied to the keyboard shortcuts as
    /// well as the buttons so the two cannot disagree.
    pub const ZOOM_MIN: f32 = 0.6;
    pub const ZOOM_MAX: f32 = 3.0;
    const ZOOM_STEP: f32 = 0.1;

    /// The largest window that fits `monitor`, never larger than `desired`.
    ///
    /// Shrink-only by construction: a window already smaller than its share of
    /// the monitor is returned untouched, so this cannot fight a display whose
    /// size is unknown, misreported, or simply generous.
    pub fn fit_to_monitor(desired: [f32; 2], monitor: Option<[f32; 2]>) -> [f32; 2] {
        let Some([monitor_width, monitor_height]) = monitor else {
            return desired;
        };
        let [width, height] = desired;
        [
            fit_axis(width, monitor_width, FIT[0], MIN_SIZE[0]),
            fit_axis(height, monitor_height, FIT[1], MIN_SIZE[1]),
        ]
    }

    /// One axis of `fit_to_monitor`. A monitor that reports nothing usable
    /// leaves the request alone rather than guessing.
    fn fit_axis(desired: f32, monitor: f32, fraction: f32, floor: f32) -> f32 {
        if !desired.is_finite() {
            return floor;
        }
        if monitor.is_finite() && monitor > 0.0 {
            desired.min(floor.max(monitor * fraction))
        } else {
            desired
        }
    }

    /// Width of the centred form column inside `available` points.
    pub fn column_width(available: f32) -> f32 {
        if available.is_finite() {
            available.clamp(0.0, COLUMN_MAX)
        } else {
            COLUMN_MAX
        }
    }

    /// Whether a column of this width puts labels beside their fields.
    pub fn fields_paired(column: f32) -> bool {
        column >= PAIRED_MIN
    }

    /// `current` moved `steps` notches, rounded to a whole percent-of-ten and
    /// held inside the bounds. The one zoom rule: buttons and keyboard
    /// shortcuts both go through it.
    pub fn step_zoom(current: f32, steps: i8) -> f32 {
        let base = if current.is_finite() { current } else { 1.0 };
        let stepped = base + f32::from(steps) * ZOOM_STEP;
        ((stepped * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
    }

    #[cfg(test)]
    mod tests {
        use super::{
            COLUMN_MAX, DEFAULT_SIZE, LABEL_WIDTH, MIN_SIZE, PAIRED_MIN, ZOOM_MAX, ZOOM_MIN,
            column_width, fields_paired, fit_to_monitor, step_zoom,
        };

        fn close(left: f32, right: f32) -> bool {
            (left - right).abs() < 1e-6
        }

        fn same_size(left: [f32; 2], right: [f32; 2]) -> bool {
            close(left[0], right[0]) && close(left[1], right[1])
        }

        /// The case the old 720x480 minimum could not express: a 1366x768
        /// panel at 200% scaling is 683x384 points, and a window larger than
        /// that puts its buttons off the screen with no way to shrink it.
        #[test]
        fn a_window_opens_inside_a_heavily_scaled_panel() {
            let [width, height] = fit_to_monitor(DEFAULT_SIZE, Some([683.0, 384.0]));
            assert!(width <= 683.0 && height <= 384.0);
            assert!(width >= MIN_SIZE[0] && height >= MIN_SIZE[1]);
        }

        #[test]
        fn fitting_only_ever_shrinks() {
            for monitor in [
                Some([3840.0, 2160.0]),
                Some([1920.0, 1080.0]),
                Some([683.0, 384.0]),
                Some([320.0, 240.0]),
                None,
            ] {
                let [width, height] = fit_to_monitor(DEFAULT_SIZE, monitor);
                assert!(width <= DEFAULT_SIZE[0] && height <= DEFAULT_SIZE[1]);
            }
            // A desktop with room to spare gets the window as asked for.
            assert!(same_size(
                fit_to_monitor(DEFAULT_SIZE, Some([3840.0, 2160.0])),
                DEFAULT_SIZE
            ));
        }

        /// A monitor smaller than the floor still yields the floor, and a
        /// window already below it is left alone rather than grown.
        #[test]
        fn the_floor_holds_without_growing_a_small_window() {
            assert!(same_size(
                fit_to_monitor(DEFAULT_SIZE, Some([300.0, 200.0])),
                MIN_SIZE
            ));
            assert!(same_size(
                fit_to_monitor([360.0, 300.0], Some([300.0, 200.0])),
                [360.0, 300.0]
            ));
        }

        #[test]
        fn an_unreadable_monitor_size_changes_nothing() {
            for monitor in [
                None,
                Some([0.0, 0.0]),
                Some([f32::NAN, f32::NAN]),
                Some([f32::INFINITY, f32::INFINITY]),
                Some([-1920.0, -1080.0]),
            ] {
                assert!(same_size(
                    fit_to_monitor(DEFAULT_SIZE, monitor),
                    DEFAULT_SIZE
                ));
            }
        }

        #[test]
        fn the_form_column_stops_growing_but_never_overflows() {
            assert!(close(column_width(3840.0), COLUMN_MAX));
            assert!(close(column_width(400.0), 400.0));
            assert!(close(column_width(-10.0), 0.0));
            assert!(close(column_width(f32::NAN), COLUMN_MAX));
        }

        /// The narrowest window and the paired layout have to agree: at
        /// `MIN_SIZE` the labels stack, and a paired row leaves the field more
        /// room than the label gutter it sits beside.
        #[test]
        fn labels_pair_only_where_there_is_room_for_both() {
            assert!(fields_paired(COLUMN_MAX));
            assert!(fields_paired(PAIRED_MIN));
            assert!(!fields_paired(PAIRED_MIN - 1.0));
            assert!(!fields_paired(column_width(MIN_SIZE[0])));
            const { assert!(PAIRED_MIN - LABEL_WIDTH > LABEL_WIDTH) }
        }

        #[test]
        fn zoom_steps_by_a_tenth_and_saturates_at_both_bounds() {
            assert!(close(step_zoom(1.0, 1), 1.1));
            assert!(close(step_zoom(1.0, -1), 0.9));
            assert!(close(step_zoom(1.0, 5), 1.5));
            let mut zoom = 1.0;
            for _ in 0..100 {
                zoom = step_zoom(zoom, -1);
            }
            assert!(close(zoom, ZOOM_MIN));
            for _ in 0..100 {
                zoom = step_zoom(zoom, 1);
            }
            assert!(close(zoom, ZOOM_MAX));
        }

        /// Repeated stepping has to land back on whole tenths, or the readout
        /// drifts to "Zoom 110.000002%" over a session.
        #[test]
        fn zoom_returns_to_exactly_one() {
            let mut zoom = 1.0;
            for _ in 0..8 {
                zoom = step_zoom(zoom, 1);
            }
            for _ in 0..8 {
                zoom = step_zoom(zoom, -1);
            }
            assert!(close(zoom, 1.0));
            assert!(close(step_zoom(f32::NAN, 1), 1.1));
        }
    }
}

/// The Activity view's backing log.
///
/// Outside the `desktop` module for the same reason as `gates`: "how many lines
/// are kept" is a rule, not a widget, and one that only shows up after a long
/// session. Compiled and tested without the feature so it cannot go unchecked.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod activity {
    /// Lines kept on screen. `Logs` alone appends up to 500 per press, and
    /// nothing used to remove one, so a long session grew the log without
    /// bound -- the last unbounded buffer in the deployer.
    pub const LIMIT: usize = 2000;

    /// A bounded, append-only view log that drops its oldest lines when full.
    #[derive(Default)]
    pub struct Log {
        lines: Vec<String>,
    }

    impl Log {
        pub fn push(&mut self, line: String) {
            if self.lines.len() >= LIMIT {
                // Keep the newest LIMIT - 1, so the arrival below lands inside
                // the cap rather than one past it.
                self.lines.drain(..=(self.lines.len() - LIMIT));
            }
            self.lines.push(line);
        }

        /// Lines currently held, for the view's row count. Not `len`, which
        /// would owe the type an `is_empty` no caller wants.
        pub fn line_count(&self) -> usize {
            self.lines.len()
        }

        /// The lines in `rows`, for a view that draws only what is on screen.
        /// A range past the end yields nothing rather than panicking: the row
        /// count the scroll area was given is a frame older than this call,
        /// and a trim between the two must not take the window down.
        pub fn rows(&self, rows: std::ops::Range<usize>) -> &[String] {
            self.lines.get(rows).unwrap_or_default()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{LIMIT, Log};

        #[test]
        fn the_log_keeps_the_newest_lines_and_never_exceeds_its_cap() {
            let mut log = Log::default();
            for index in 0..LIMIT + 500 {
                log.push(index.to_string());
            }
            let lines = log.rows(0..log.line_count());
            assert_eq!(lines.len(), LIMIT);
            // The oldest 500 were dropped; the newest is the last one pushed.
            assert_eq!(lines.first().map(String::as_str), Some("500"));
            assert_eq!(
                lines.last().map(String::as_str),
                Some((LIMIT + 499).to_string().as_str())
            );
        }

        #[test]
        fn a_short_log_is_untouched() {
            let mut log = Log::default();
            log.push("first".into());
            log.push("second".into());
            assert_eq!(log.line_count(), 2);
            assert_eq!(log.rows(0..2).first().map(String::as_str), Some("first"));
        }

        /// The view draws only the rows on screen, so it asks for a window of
        /// them by index. A stale range must not be a panic: the count it was
        /// given came from the previous frame.
        #[test]
        fn a_window_of_rows_survives_a_stale_range() {
            let mut log = Log::default();
            for index in 0..5 {
                log.push(index.to_string());
            }
            assert_eq!(log.line_count(), 5);
            assert_eq!(log.rows(1..3), ["1".to_owned(), "2".to_owned()]);
            assert_eq!(log.rows(0..5).len(), 5);
            assert!(log.rows(3..9).is_empty());
            assert!(log.rows(9..9).is_empty());
            assert!(Log::default().rows(0..1).is_empty());
        }
    }
}

#[cfg(feature = "desktop")]
mod desktop {
    use crate::activity::Log;
    use crate::gates::Form;
    use crate::layout;
    use eframe::egui;
    use omt_deployer_core::{
        AuthMethod, Connection, DeployOptions, ManagementAction, Secret, WifiSettings, apply_wifi,
        deploy, discover_project_root, manage, test_connection, validate_wifi,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread;
    use zeroize::Zeroizing;

    const LICENSE: &str = include_str!("../../../LICENSE");
    const NOTICES: &str = include_str!("../../../THIRD_PARTY_NOTICES.txt");
    const VERSION: &str = match option_env!("RPI_OMT_CLIENT_VERSION") {
        Some(value) => value,
        None => env!("CARGO_PKG_VERSION"),
    };

    #[derive(Clone, Copy, PartialEq)]
    enum View {
        Connection,
        Deploy,
        Manage,
        Wifi,
        Activity,
        About,
    }

    enum WorkerEvent {
        Line(String),
        Finished(Result<(), String>),
    }

    enum Job {
        Test,
        Deploy,
        Manage(ManagementAction),
        Wifi,
    }

    pub struct App {
        view: View,
        host: String,
        user: String,
        password: Zeroizing<String>,
        wifi_ssid: String,
        wifi_password: Zeroizing<String>,
        wifi_connect: bool,
        project_root: String,
        remote_directory: String,
        reveal: bool,
        activity: Log,
        cancel: Arc<AtomicBool>,
        running: bool,
        events: Option<Receiver<WorkerEvent>>,
        fit: Fit,
    }

    /// Whether the opening window has been fitted to the display it landed on.
    /// Settled on the first frame that names a monitor, because that is the
    /// first moment the display behind the window is known.
    #[derive(PartialEq, Eq)]
    enum Fit {
        Pending,
        Settled,
    }

    impl Default for App {
        fn default() -> Self {
            let starts = [
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from(".")),
            ];
            let project = discover_project_root(&starts)
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            Self {
                view: View::Connection,
                host: "raspberrypi.local".into(),
                user: "root".into(),
                password: Zeroizing::new(String::new()),
                wifi_ssid: String::new(),
                wifi_password: Zeroizing::new(String::new()),
                wifi_connect: true,
                project_root: project,
                remote_directory: "/opt/omt-client".into(),
                reveal: false,
                activity: Log::default(),
                cancel: Arc::new(AtomicBool::new(false)),
                running: false,
                events: None,
                fit: Fit::Pending,
            }
        }
    }

    impl App {
        /// The form as typed, for the gating rules the core owns.
        fn form(&self) -> Form<'_> {
            Form {
                host: &self.host,
                user: &self.user,
                password: &self.password,
                project_root: &self.project_root,
                wifi_ssid: &self.wifi_ssid,
                wifi_password: &self.wifi_password,
            }
        }

        fn connection(&self) -> Result<Connection, String> {
            let password =
                Secret::new((*self.password).clone()).map_err(|error| error.to_string())?;
            let connection = Connection {
                host: self.host.clone(),
                username: self.user.clone(),
                port: 22,
                auth: AuthMethod::Password,
                password: Some(password),
                key_path: None,
                key_passphrase: None,
                sudo_password: None,
            };
            omt_deployer_core::validate_connection(&connection)
                .map_err(|error| error.to_string())?;
            Ok(connection)
        }

        fn start_job(&mut self, job: Job) {
            if self.running {
                return;
            }
            let connection = match self.connection() {
                Ok(value) => value,
                Err(error) => {
                    self.activity.push(error);
                    self.view = View::Activity;
                    return;
                }
            };
            let request = JobRequest {
                job,
                connection,
                project_root: PathBuf::from(&self.project_root),
                remote_directory: self.remote_directory.clone(),
                wifi_ssid: self.wifi_ssid.clone(),
                wifi_password: self.wifi_password.clone(),
                wifi_connect: self.wifi_connect,
            };
            let (tx, rx) = mpsc::channel();
            self.cancel.store(false, Ordering::Relaxed);
            let cancel = Arc::clone(&self.cancel);
            self.events = Some(rx);
            self.running = true;
            self.view = View::Activity;
            self.activity.push(match request.job {
                Job::Test => "Testing connection...".into(),
                Job::Deploy => "Starting deployment...".into(),
                Job::Manage(ManagementAction::Status) => "Fetching status...".into(),
                Job::Manage(ManagementAction::Logs) => "Fetching logs...".into(),
                Job::Manage(ManagementAction::Restart) => "Restarting service...".into(),
                Job::Wifi => "Applying Wi-Fi settings...".into(),
            });
            thread::spawn(move || {
                let result = run_job(request, &cancel, &tx);
                let _ = tx.send(WorkerEvent::Finished(result));
            });
        }

        fn poll_worker(&mut self, context: &egui::Context) {
            let Some(events) = self.events.as_ref() else {
                return;
            };
            let mut finished = None;
            while let Ok(event) = events.try_recv() {
                match event {
                    WorkerEvent::Line(line) => self.activity.push(line),
                    WorkerEvent::Finished(result) => finished = Some(result),
                }
            }
            if let Some(result) = finished {
                match result {
                    Ok(()) => self.activity.push("Operation completed.".into()),
                    Err(error) => self.activity.push(format!("ERROR: {error}")),
                }
                self.running = false;
                self.events = None;
                self.cancel.store(false, Ordering::Relaxed);
            } else if self.running {
                context.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        /// Bring the opening window inside the display it landed on.
        ///
        /// eframe clamps the requested size against the *largest* monitor and
        /// with no margin, which is the wrong monitor on a mixed-DPI desk and
        /// the wrong size under a task bar. The first frame is the first point
        /// at which the window's own monitor is known, so the fit happens
        /// here, once, and only ever shrinks.
        fn fit_window(&mut self, context: &egui::Context) {
            if self.fit == Fit::Settled {
                return;
            }
            // Already in egui points: egui-winit divides the monitor's
            // physical size by the window's pixels-per-point. A backend that
            // does not name a monitor yet leaves the fit pending rather than
            // spending it on a guess.
            let Some(monitor) = context.input(|input| input.viewport().monitor_size) else {
                return;
            };
            self.fit = Fit::Settled;
            let current = context.screen_rect().size();
            let fitted =
                layout::fit_to_monitor([current.x, current.y], Some([monitor.x, monitor.y]));
            // A point of slack: resizing to what the window already is costs a
            // needless round trip to the compositor on every launch.
            if fitted[0] < current.x - 1.0 || fitted[1] < current.y - 1.0 {
                context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    fitted[0], fitted[1],
                )));
            }
        }

        /// Apply the zoom shortcuts through the same rule the buttons use.
        ///
        /// egui would handle these itself, but with its own 0.2-to-5.0 clamp,
        /// which is how the keyboard would come to reach a zoom the buttons
        /// refuse -- the mistake `gates` exists to prevent for the Wi-Fi
        /// button. So its handler is turned off and the shortcuts are consumed
        /// here.
        fn zoom_input(context: &egui::Context) {
            use egui::gui_zoom::kb_shortcuts;
            context.options_mut(|options| options.zoom_with_keyboard = false);
            let (reset, steps) = context.input_mut(|input| {
                let reset = input.consume_shortcut(&kb_shortcuts::ZOOM_RESET);
                let mut steps: i8 = 0;
                if input.consume_shortcut(&kb_shortcuts::ZOOM_IN)
                    || input.consume_shortcut(&kb_shortcuts::ZOOM_IN_SECONDARY)
                {
                    steps += 1;
                }
                if input.consume_shortcut(&kb_shortcuts::ZOOM_OUT) {
                    steps -= 1;
                }
                (reset, steps)
            });
            if reset {
                context.set_zoom_factor(1.0);
            } else if steps != 0 {
                context.set_zoom_factor(layout::step_zoom(context.zoom_factor(), steps));
            }
        }

        /// The bar that reports what the deployer made of the display, and
        /// lets the operator overrule it.
        fn status_bar(&self, ui: &mut egui::Ui) {
            let context = ui.ctx().clone();
            let zoom = context.zoom_factor();
            let scale = context
                .input(|input| input.viewport().native_pixels_per_point)
                .unwrap_or(1.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Display scale {:.0}%", scale * 100.0));
                ui.separator();
                ui.label(format!("Zoom {:.0}%", zoom * 100.0));
                if ui
                    .add_enabled(zoom > layout::ZOOM_MIN, egui::Button::new("-"))
                    .on_hover_text(format!(
                        "Zoom out ({})",
                        context.format_shortcut(&egui::gui_zoom::kb_shortcuts::ZOOM_OUT)
                    ))
                    .clicked()
                {
                    context.set_zoom_factor(layout::step_zoom(zoom, -1));
                }
                if ui
                    .add_enabled(zoom < layout::ZOOM_MAX, egui::Button::new("+"))
                    .on_hover_text(format!(
                        "Zoom in ({})",
                        context.format_shortcut(&egui::gui_zoom::kb_shortcuts::ZOOM_IN)
                    ))
                    .clicked()
                {
                    context.set_zoom_factor(layout::step_zoom(zoom, 1));
                }
                if ui
                    .add_enabled(
                        (zoom - 1.0).abs() > f32::EPSILON,
                        egui::Button::new("Reset"),
                    )
                    .on_hover_text(format!(
                        "Reset zoom ({})",
                        context.format_shortcut(&egui::gui_zoom::kb_shortcuts::ZOOM_RESET)
                    ))
                    .clicked()
                {
                    context.set_zoom_factor(1.0);
                }
                ui.separator();
                ui.label(if self.running { "Busy" } else { "Idle" });
            });
        }
    }

    /// Run `body` in a scrolling, centred column of readable width.
    ///
    /// Every view goes through this: the scroll is what keeps a button
    /// reachable when the window is at its minimum, and the width cap is what
    /// stops a host-name field spanning a 4K desktop.
    fn column(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let width = layout::column_width(ui.available_width());
                let inset = (ui.available_width() - width) / 2.0;
                ui.horizontal(|ui| {
                    ui.add_space(inset);
                    ui.vertical(|ui| {
                        ui.set_max_width(width);
                        body(ui);
                    });
                });
            });
    }

    /// One labelled field: beside its label where the column is wide enough
    /// for both, stacked above it where it is not.
    fn field(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
        if layout::fields_paired(ui.max_rect().width()) {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [layout::LABEL_WIDTH, ui.spacing().interact_size.y],
                    egui::Label::new(label).halign(egui::Align::LEFT),
                );
                body(ui);
            });
        } else {
            ui.label(label);
            body(ui);
        }
        ui.add_space(ui.spacing().item_spacing.y);
    }

    /// A text field that fills the column rather than the window.
    fn text_field(ui: &mut egui::Ui, text: &mut String, secret: bool) {
        ui.add(
            egui::TextEdit::singleline(text)
                .password(secret)
                .desired_width(f32::INFINITY),
        );
    }

    /// One queued operation, handed to the worker thread.
    ///
    /// `wifi_password` stays `Zeroizing` across the move: the form field it is
    /// cloned from is zeroized, and copying it into a bare `String` for the
    /// trip through the channel would defeat that for every request that is
    /// built but never sent to `apply_wifi`.
    struct JobRequest {
        job: Job,
        connection: Connection,
        project_root: PathBuf,
        remote_directory: String,
        wifi_ssid: String,
        wifi_password: Zeroizing<String>,
        wifi_connect: bool,
    }

    fn run_job(
        request: JobRequest,
        cancel: &Arc<AtomicBool>,
        tx: &Sender<WorkerEvent>,
    ) -> Result<(), String> {
        let mut progress = |message: &str| {
            for line in message.lines() {
                if !line.is_empty() {
                    let _ = tx.send(WorkerEvent::Line(line.to_owned()));
                }
            }
        };
        match request.job {
            Job::Test => test_connection(&request.connection, cancel, &mut progress)
                .map_err(|error| error.to_string()),
            Job::Deploy => {
                let options = DeployOptions {
                    project_root: request.project_root,
                    remote_directory: request.remote_directory,
                    tarball_name: "omt-client-arm64.tar.gz".into(),
                    build_image: true,
                };
                deploy(&request.connection, &options, cancel, &mut progress)
                    .map_err(|error| error.to_string())
            }
            Job::Manage(action) => manage(&request.connection, action, cancel, &mut progress)
                .map(|_| ())
                .map_err(|error| error.to_string()),
            Job::Wifi => {
                let settings = WifiSettings {
                    ssid: request.wifi_ssid,
                    password: Secret::new((*request.wifi_password).clone())
                        .map_err(|error| error.to_string())?,
                    connect: request.wifi_connect,
                };
                validate_wifi(&settings).map_err(|error| error.to_string())?;
                apply_wifi(&request.connection, &settings, cancel, &mut progress)
                    .map_err(|error| error.to_string())
            }
        }
    }

    impl eframe::App for App {
        fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
            self.poll_worker(context);
            self.fit_window(context);
            Self::zoom_input(context);
            egui::TopBottomPanel::top("navigation").show(context, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (name, view) in [
                        ("Connection", View::Connection),
                        ("Deploy", View::Deploy),
                        ("Manage", View::Manage),
                        ("Wi-Fi", View::Wifi),
                        ("Activity", View::Activity),
                        ("About", View::About),
                    ] {
                        if ui.selectable_label(self.view == view, name).clicked() {
                            self.view = view;
                        }
                    }
                });
            });
            egui::TopBottomPanel::bottom("status").show(context, |ui| self.status_bar(ui));
            egui::CentralPanel::default().show(context, |ui| match self.view {
                View::Connection => column(ui, |ui| {
                    ui.heading("Connection");
                    field(ui, "Pi host", |ui| text_field(ui, &mut self.host, false));
                    field(ui, "SSH username", |ui| {
                        text_field(ui, &mut self.user, false);
                    });
                    field(ui, "SSH password", |ui| {
                        text_field(ui, &mut self.password, !self.reveal);
                    });
                    ui.checkbox(&mut self.reveal, "Reveal secrets");
                    ui.add_space(ui.spacing().item_spacing.y);
                    let enabled = self.form().can_connect() && !self.running;
                    if ui
                        .add_enabled(enabled, egui::Button::new("Test connection"))
                        .clicked()
                    {
                        self.start_job(Job::Test);
                    }
                }),
                View::Deploy => column(ui, |ui| {
                    ui.heading("Deploy");
                    ui.label(
                        "Build, verify, upload, recover, and promote the manifest-v3 capsule.",
                    );
                    ui.add_space(ui.spacing().item_spacing.y);
                    field(ui, "Project root", |ui| {
                        text_field(ui, &mut self.project_root, false);
                    });
                    field(ui, "Remote directory", |ui| {
                        text_field(ui, &mut self.remote_directory, false);
                    });
                    let enabled = self.form().can_deploy() && !self.running;
                    if ui
                        .add_enabled(enabled, egui::Button::new("Deploy"))
                        .clicked()
                    {
                        self.start_job(Job::Deploy);
                    }
                }),
                View::Manage => column(ui, |ui| {
                    ui.heading("Manage");
                    // Wrapped, so three buttons do not run off a narrow window.
                    ui.horizontal_wrapped(|ui| {
                        for (label, action) in [
                            ("Status", ManagementAction::Status),
                            ("Logs", ManagementAction::Logs),
                            ("Restart", ManagementAction::Restart),
                        ] {
                            if ui
                                .add_enabled(!self.running, egui::Button::new(label))
                                .clicked()
                            {
                                self.start_job(Job::Manage(action));
                            }
                        }
                    });
                }),
                View::Wifi => column(ui, |ui| {
                    ui.heading("Wi-Fi");
                    field(ui, "SSID", |ui| text_field(ui, &mut self.wifi_ssid, false));
                    field(ui, "Passphrase", |ui| {
                        text_field(ui, &mut self.wifi_password, !self.reveal);
                    });
                    ui.checkbox(&mut self.wifi_connect, "Connect after saving");
                    ui.add_space(ui.spacing().item_spacing.y);
                    let enabled = self.form().can_apply_wifi() && !self.running;
                    if ui
                        .add_enabled(enabled, egui::Button::new("Apply Wi-Fi"))
                        .clicked()
                    {
                        self.start_job(Job::Wifi);
                    }
                }),
                View::Activity => {
                    // Full width, and with Cancel above the log rather than in
                    // it: a control that scrolls away with the output is one
                    // an operator cannot reach when it matters.
                    ui.horizontal_wrapped(|ui| {
                        ui.heading("Activity");
                        if self.running && ui.button("Cancel").clicked() {
                            self.cancel.store(true, Ordering::Relaxed);
                            self.activity.push("Cancellation requested...".into());
                        }
                    });
                    ui.separator();
                    // Only the rows on screen are laid out. The log holds up
                    // to `activity::LIMIT` lines, and laying out all of them
                    // every frame is felt first on the large, high-density
                    // displays this view is most likely to be read on.
                    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                    let rows = self.activity.line_count();
                    egui::ScrollArea::both()
                        .auto_shrink([false; 2])
                        .stick_to_bottom(true)
                        .show_rows(ui, row_height, rows, |ui, range| {
                            for line in self.activity.rows(range) {
                                // Uniform row height is what lets the scroll
                                // area skip the rows it is not drawing, so
                                // long lines extend and scroll sideways rather
                                // than wrapping to two.
                                ui.add(
                                    egui::Label::new(egui::RichText::new(line).monospace())
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            }
                        });
                }
                View::About => {
                    ui.heading("Raspberry Pi OMT Deployer");
                    ui.label(VERSION);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.collapsing("License", |ui| {
                                ui.monospace(LICENSE);
                            });
                            ui.collapsing("Third-party notices", |ui| {
                                ui.monospace(NOTICES);
                            });
                        });
                }
            });
        }
    }

    pub fn run() -> eframe::Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(layout::DEFAULT_SIZE)
                .with_min_inner_size(layout::MIN_SIZE)
                // egui's own default for this is platform-dependent, so it is
                // stated rather than inherited: a window larger than the
                // monitor is unusable everywhere and crashes some Linux
                // compositors. `App::fit_window` then refines the result
                // against the monitor the window actually opened on.
                .with_clamp_size_to_monitor_size(true),
            centered: true,
            ..eframe::NativeOptions::default()
        };
        eframe::run_native(
            "Raspberry Pi OMT Deployer",
            options,
            Box::new(|_| Ok(Box::<App>::default())),
        )
    }
}

#[cfg(feature = "desktop")]
fn main() -> eframe::Result<()> {
    desktop::run()
}
#[cfg(not(feature = "desktop"))]
fn main() {
    eprintln!("rpi-omt-deployer was built without the `desktop` feature");
}
