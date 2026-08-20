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
    use omt_deployer_core::{
        Secret, WifiSettings, valid_appliance_hostname, valid_host, valid_username,
        validate_os_password, validate_web_password, validate_wifi,
    };

    /// The connection and deployment fields as the operator has typed them.
    pub struct Form<'a> {
        pub host: &'a str,
        pub user: &'a str,
        pub password: &'a str,
        pub hostname: &'a str,
        /// The rename typed on Manage. Separate from `hostname`, which names a
        /// factory image Alpine setup is about to install: an operator who
        /// filled that in weeks ago must not be able to rename a live
        /// appliance by pressing a button on a different view.
        pub manage_hostname: &'a str,
        pub os_root_password: &'a str,
        pub os_root_password_confirmation: &'a str,
        pub os_pi_password: &'a str,
        pub os_pi_password_confirmation: &'a str,
        pub wifi_ssid: &'a str,
        pub wifi_password: &'a str,
        pub wifi_connect: bool,
        pub wifi_preserve_existing_profiles: bool,
        pub rotate_web_password: bool,
        pub web_password: &'a str,
        pub web_password_confirmation: &'a str,
    }

    impl Form<'_> {
        /// Everything `omt_deployer_core::connect` will insist on.
        ///
        /// An empty SSH password is valid: a factory Alpine image answers as
        /// root with no password until Alpine setup has run.
        pub fn can_connect(&self) -> bool {
            valid_host(self.host)
                && valid_username(self.user)
                && Secret::new(self.password.to_owned()).is_ok()
        }

        /// A deployment needs no local files, so the connection is all there
        /// is to check: the capsule it uploads is part of this program.
        pub fn can_deploy(&self) -> bool {
            self.can_connect() && (!self.rotate_web_password || self.web_password_is_ready())
        }

        pub fn can_install_alpine(&self) -> bool {
            self.can_connect()
                && valid_appliance_hostname(self.hostname)
                && self.os_passwords_are_ready()
                && self.alpine_wifi_is_ready()
        }

        pub fn can_apply_wifi(&self) -> bool {
            self.can_connect()
                && Secret::new(self.wifi_password.to_owned()).is_ok_and(|password| {
                    validate_wifi(&WifiSettings {
                        ssid: self.wifi_ssid.to_owned(),
                        password,
                        connect: self.wifi_connect,
                        preserve_existing_profiles: self.wifi_preserve_existing_profiles,
                    })
                    .is_ok()
                })
        }

        pub fn can_change_web_password(&self) -> bool {
            self.can_connect() && self.web_password_is_ready()
        }

        pub fn can_set_hostname(&self) -> bool {
            self.can_connect() && valid_appliance_hostname(self.manage_hostname)
        }

        fn web_password_is_ready(&self) -> bool {
            self.web_password == self.web_password_confirmation
                && Secret::new(self.web_password.to_owned())
                    .is_ok_and(|password| validate_web_password(&password).is_ok())
        }

        fn os_passwords_are_ready(&self) -> bool {
            self.os_root_password == self.os_root_password_confirmation
                && self.os_pi_password == self.os_pi_password_confirmation
                && Secret::new(self.os_root_password.to_owned())
                    .is_ok_and(|password| validate_os_password(&password).is_ok())
                && Secret::new(self.os_pi_password.to_owned())
                    .is_ok_and(|password| validate_os_password(&password).is_ok())
        }

        fn alpine_wifi_is_ready(&self) -> bool {
            if self.wifi_ssid.is_empty() && self.wifi_password.is_empty() {
                return true;
            }
            Secret::new(self.wifi_password.to_owned()).is_ok_and(|password| {
                validate_wifi(&WifiSettings {
                    ssid: self.wifi_ssid.to_owned(),
                    password,
                    connect: false,
                    preserve_existing_profiles: true,
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
                hostname: "omt-client",
                manage_hostname: "studio-pi-2",
                os_root_password: "rootpass1",
                os_root_password_confirmation: "rootpass1",
                os_pi_password: "pipassword",
                os_pi_password_confirmation: "pipassword",
                wifi_ssid,
                wifi_password,
                wifi_connect: true,
                wifi_preserve_existing_profiles: true,
                rotate_web_password: false,
                web_password: "correct horse battery staple",
                web_password_confirmation: "correct horse battery staple",
            }
        }

        #[test]
        fn connection_fields_gate_every_action() {
            let complete = form("", "");
            assert!(complete.can_connect());
            assert!(complete.can_deploy());
            assert!(complete.can_install_alpine());
            assert!(complete.can_change_web_password());
            for incomplete in [
                Form {
                    host: "-pi.local",
                    ..form("", "")
                },
                Form {
                    user: "ro ot",
                    ..form("", "")
                },
            ] {
                assert!(!incomplete.can_connect());
                assert!(!incomplete.can_deploy());
                assert!(!incomplete.can_install_alpine());
                assert!(!incomplete.can_apply_wifi());
                assert!(!incomplete.can_change_web_password());
                assert!(!incomplete.can_set_hostname());
            }
            assert!(
                Form {
                    password: "",
                    ..form("", "")
                }
                .can_connect()
            );
        }

        /// The capsule is compiled in, so a deployment asks for nothing on
        /// this machine. Anything else gating Deploy would be a local
        /// requirement the operator cannot satisfy and does not need.
        #[test]
        fn deploying_needs_only_a_connection() {
            let connected = form("", "");
            assert!(connected.can_deploy());
            assert!(
                !Form {
                    rotate_web_password: true,
                    web_password: "short",
                    web_password_confirmation: "short",
                    ..form("", "")
                }
                .can_deploy()
            );
        }

        #[test]
        fn alpine_setup_requires_hostname_and_host_passwords() {
            assert!(form("", "").can_install_alpine());
            assert!(
                !Form {
                    hostname: "-bad",
                    ..form("", "")
                }
                .can_install_alpine()
            );
            assert!(
                !Form {
                    os_root_password: "short",
                    os_root_password_confirmation: "short",
                    ..form("", "")
                }
                .can_install_alpine()
            );
            assert!(
                !Form {
                    os_pi_password_confirmation: "mismatch1",
                    ..form("", "")
                }
                .can_install_alpine()
            );
            assert!(form("studio", "passphrase").can_install_alpine());
            assert!(!form("studio", "short").can_install_alpine());
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

        #[test]
        fn the_web_password_button_requires_policy_and_confirmation() {
            assert!(form("", "").can_change_web_password());
            assert!(
                !Form {
                    web_password: "too-short",
                    web_password_confirmation: "too-short",
                    ..form("", "")
                }
                .can_change_web_password()
            );
            assert!(
                !Form {
                    web_password_confirmation: "a different secure password",
                    ..form("", "")
                }
                .can_change_web_password()
            );
        }

        /// Renaming reads its own field. Alpine setup's hostname is for a
        /// factory image that has not been installed yet, and a rename that
        /// took its value would quietly apply whatever was left in that box.
        #[test]
        fn the_rename_button_reads_the_manage_field_only() {
            assert!(form("", "").can_set_hostname());
            for rejected in ["", "-bad", "bad-", "has space", "has.dot", &"n".repeat(64)] {
                assert!(
                    !Form {
                        manage_hostname: rejected,
                        ..form("", "")
                    }
                    .can_set_hostname(),
                    "accepted an invalid appliance hostname: {rejected}"
                );
            }
            assert!(
                !Form {
                    manage_hostname: "",
                    hostname: "still-valid",
                    ..form("", "")
                }
                .can_set_hostname()
            );
        }

        #[test]
        fn deploy_leaves_the_web_password_alone_unless_rotation_is_enabled() {
            let off = form("", "");
            assert!(!off.rotate_web_password);
            assert!(off.can_deploy());
            assert!(
                Form {
                    web_password: "",
                    web_password_confirmation: "",
                    ..form("", "")
                }
                .can_deploy()
            );
            assert!(
                !Form {
                    rotate_web_password: true,
                    web_password: "",
                    web_password_confirmation: "",
                    ..form("", "")
                }
                .can_deploy()
            );
            assert!(
                Form {
                    rotate_web_password: true,
                    ..form("", "")
                }
                .can_deploy()
            );
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

    /// How long after the window's own monitor is first known the opening fit
    /// may keep asking the compositor to shrink.
    ///
    /// A resize is a round trip: the frames right after the request still
    /// report the old size, so a single attempt cannot tell "not applied yet"
    /// from "refused". Retrying covers a dropped request. The budget is wall
    /// time, not a frame count, because `request_repaint` can produce frames
    /// far faster than the compositor applies `InnerSize`; counting those
    /// would settle while the window is still oversized. The bound also stops
    /// a compositor that simply will not resize -- a tiling one -- from being
    /// asked for the life of the process.
    pub const OPENING_FIT_BUDGET: std::time::Duration = std::time::Duration::from_millis(750);

    /// Slack in points before a change of outer origin counts as a move.
    /// Sub-pixel compositor jitter must not spend the opening fit.
    const MOVE_SLACK: f32 = 2.0;

    /// Whether `current` is still bigger than the monitor allows. A point of
    /// slack: asking the compositor for what the window already is costs a
    /// round trip and can itself perturb the window.
    pub fn needs_shrink(current: [f32; 2], fitted: [f32; 2]) -> bool {
        fitted[0] < current[0] - 1.0 || fitted[1] < current[1] - 1.0
    }

    /// Whether the window's outer origin has left the place it first appeared.
    pub fn origin_moved(origin: [f32; 2], current: [f32; 2]) -> bool {
        (origin[0] - current[0]).abs() > MOVE_SLACK || (origin[1] - current[1]).abs() > MOVE_SLACK
    }

    /// Whether the opening-fit budget has been spent. After this, a newly
    /// reported monitor is a display the window was dragged onto.
    pub fn opening_fit_budget_expired(elapsed: std::time::Duration) -> bool {
        elapsed > OPENING_FIT_BUDGET
    }

    /// What the opening fit should do on a frame where the window's own
    /// monitor is known and the window has not been dragged.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum FitStep {
        /// Ask the compositor for this size, and look again next frame.
        Shrink([f32; 2]),
        /// The window fits, the budget ran out, or the compositor will not
        /// shrink it. Either way there is nothing further to ask for.
        Done,
    }

    /// The opening fit's whole decision, kept here so that "when does the
    /// window stop being resized" is a rule with a test rather than something
    /// only reproducible on a particular desk.
    pub fn fit_step(current: [f32; 2], monitor: [f32; 2], elapsed: std::time::Duration) -> FitStep {
        if opening_fit_budget_expired(elapsed) {
            return FitStep::Done;
        }
        let fitted = fit_to_monitor(current, Some(monitor));
        if !needs_shrink(current, fitted) {
            return FitStep::Done;
        }
        FitStep::Shrink(fitted)
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
            COLUMN_MAX, DEFAULT_SIZE, FitStep, LABEL_WIDTH, MIN_SIZE, OPENING_FIT_BUDGET,
            PAIRED_MIN, ZOOM_MAX, ZOOM_MIN, column_width, fields_paired, fit_step, fit_to_monitor,
            needs_shrink, opening_fit_budget_expired, origin_moved, step_zoom,
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

        /// The opening fit stops asking once the window fits, and a point of
        /// slack keeps it from asking for a size the window already has.
        #[test]
        fn the_fit_stops_when_the_window_fits() {
            assert!(needs_shrink([960.0, 640.0], [800.0, 640.0]));
            assert!(needs_shrink([960.0, 640.0], [960.0, 500.0]));
            assert!(!needs_shrink([960.0, 640.0], [960.0, 640.0]));
            assert!(!needs_shrink([960.0, 640.0], [959.5, 639.5]));
        }

        /// A drag is a real move of the outer origin, not compositor jitter,
        /// and the opening budget is spent once rather than on every later
        /// monitor the window crosses.
        #[test]
        fn opening_fit_stops_after_a_move_or_the_budget() {
            assert!(!origin_moved([100.0, 80.0], [100.5, 80.5]));
            assert!(origin_moved([100.0, 80.0], [120.0, 80.0]));
            assert!(!opening_fit_budget_expired(OPENING_FIT_BUDGET));
            assert!(opening_fit_budget_expired(
                OPENING_FIT_BUDGET + std::time::Duration::from_millis(1)
            ));
        }

        /// A resize is a round trip, so the frames right after a request still
        /// report the old size. The fit has to keep asking across that lag,
        /// stop as soon as the window fits, and still terminate against a
        /// compositor that never applies it -- on wall time, not a frame
        /// count, because a tight repaint loop can burn eight frames before
        /// the first `InnerSize` lands.
        #[test]
        fn the_fit_converges_and_always_terminates() {
            let early = std::time::Duration::from_millis(32);
            // A monitor smaller than the default window: shrink towards it.
            assert_eq!(
                fit_step(DEFAULT_SIZE, [800.0, 600.0], std::time::Duration::ZERO),
                FitStep::Shrink([720.0, 510.0])
            );
            // Still oversized later in the budget, because the resize has not
            // landed yet. The fit must ask again rather than settle.
            assert_eq!(
                fit_step(DEFAULT_SIZE, [800.0, 600.0], early),
                FitStep::Shrink([720.0, 510.0])
            );
            // Once it has landed, it is done.
            assert_eq!(
                fit_step([720.0, 510.0], [800.0, 600.0], early),
                FitStep::Done
            );
            // A monitor with room to spare is never touched at all.
            assert_eq!(
                fit_step(DEFAULT_SIZE, [3840.0, 2160.0], std::time::Duration::ZERO),
                FitStep::Done
            );
            // A compositor that refuses every request: the budget ends the
            // loop `fit_window` runs, or it resizes on every frame forever.
            assert_eq!(
                fit_step(
                    DEFAULT_SIZE,
                    [800.0, 600.0],
                    OPENING_FIT_BUDGET + std::time::Duration::from_millis(1)
                ),
                FitStep::Done
            );
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
    // Job, JobRequest, WorkerEvent, and run_job live in the core rather than
    // here: the terminal deployer runs the same jobs, and a deployment must
    // not mean something different depending on which frontend started it.
    use omt_deployer_core::{
        AuthMethod, Connection, DeployOptions, IMAGE_MEMBER, Job, JobRequest, ManagementAction,
        Secret, WorkerEvent, embedded_image, run_job,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver};
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
        Alpine,
        Deploy,
        Manage,
        Wifi,
        Activity,
        About,
    }

    /// Which field an open file dialog is filling in.
    #[derive(Clone, Copy, PartialEq)]
    enum Picking {
        KnownHosts,
    }

    #[allow(clippy::struct_excessive_bools)]
    pub struct App {
        view: View,
        host: String,
        user: String,
        password: Zeroizing<String>,
        sudo_password: Zeroizing<String>,
        known_hosts: String,
        hostname: String,
        /// The rename typed on Manage, kept apart from `hostname` so the
        /// Alpine view's factory-image name can never drive a live rename.
        manage_hostname: String,
        os_root_password: Zeroizing<String>,
        os_root_password_confirmation: Zeroizing<String>,
        os_pi_password: Zeroizing<String>,
        os_pi_password_confirmation: Zeroizing<String>,
        wifi_ssid: String,
        wifi_password: Zeroizing<String>,
        rotate_web_password: bool,
        web_password: Zeroizing<String>,
        web_password_confirmation: Zeroizing<String>,
        wifi_connect: bool,
        wifi_preserve_existing_profiles: bool,
        remote_directory: String,
        reveal: bool,
        activity: Log,
        cancel: Arc<AtomicBool>,
        events: Option<Receiver<WorkerEvent>>,
        picker: Option<(Picking, Receiver<Option<PathBuf>>)>,
        fit: Fit,
        pending_confirmation: Option<ManagementAction>,
        pending_alpine_confirm: bool,
        apply_alpine_login: bool,
    }

    /// Whether the opening window has been fitted to the display it landed on.
    ///
    /// Pending until the window is observed to fit, dragged, or the opening
    /// budget expires. `started` is the first frame the window's own monitor
    /// was known -- not process start -- so a backend that names the monitor
    /// late still gets a fit. `origin` is the outer position on that first
    /// named-monitor frame; a later move is a drag, and resizing then snaps
    /// the window back. Settled is permanent: the fit belongs to the opening,
    /// and re-running it later would resize the window out from under an
    /// operator who had dragged it somewhere deliberately.
    #[derive(Clone, Copy)]
    enum Fit {
        Pending {
            started: Option<std::time::Instant>,
            origin: Option<egui::Pos2>,
        },
        Settled,
    }

    impl Default for App {
        fn default() -> Self {
            Self {
                view: View::Connection,
                host: "raspberrypi.local".into(),
                user: "root".into(),
                password: Zeroizing::new(String::new()),
                sudo_password: Zeroizing::new(String::new()),
                known_hosts: String::new(),
                hostname: "omt-client".into(),
                manage_hostname: String::new(),
                os_root_password: Zeroizing::new(String::new()),
                os_root_password_confirmation: Zeroizing::new(String::new()),
                os_pi_password: Zeroizing::new(String::new()),
                os_pi_password_confirmation: Zeroizing::new(String::new()),
                wifi_ssid: String::new(),
                wifi_password: Zeroizing::new(String::new()),
                rotate_web_password: false,
                web_password: Zeroizing::new(String::new()),
                web_password_confirmation: Zeroizing::new(String::new()),
                wifi_connect: true,
                wifi_preserve_existing_profiles: true,
                remote_directory: "/opt/omt-client".into(),
                reveal: false,
                activity: Log::default(),
                cancel: Arc::new(AtomicBool::new(false)),
                events: None,
                picker: None,
                fit: Fit::Pending {
                    started: None,
                    origin: None,
                },
                pending_confirmation: None,
                pending_alpine_confirm: false,
                apply_alpine_login: false,
            }
        }
    }

    impl App {
        /// Whether a worker is running.
        ///
        /// Derived from the channel rather than kept beside it: the two were
        /// separate fields set and cleared together, which is one state with
        /// two ways to be wrong.
        fn running(&self) -> bool {
            self.events.is_some()
        }

        /// The deployment as the form describes it.
        ///
        /// The capsule is compiled into this application, so the only thing
        /// the form decides is where it lands. Building an image and pointing
        /// at a checkout are developer operations, and they live on the CLI.
        fn deploy_options(&self) -> DeployOptions {
            DeployOptions {
                project_root: None,
                remote_directory: self.remote_directory.clone(),
                rebuild_image: false,
            }
        }

        /// The appliance this copy of the deployer carries.
        ///
        /// Shown on Deploy and About because a single-file deployer is
        /// otherwise silent about which build it would install, and the
        /// operator has no archive on disk to look at.
        fn capsule_line() -> String {
            embedded_image().map_or_else(
                || format!("This deployer was built without {IMAGE_MEMBER}."),
                |image| {
                    format!(
                        "Carrying {IMAGE_MEMBER}, {} MiB, built into this application.",
                        image.bytes.len() / (1024 * 1024)
                    )
                },
            )
        }

        /// The form as typed, for the gating rules the core owns.
        fn form(&self) -> Form<'_> {
            Form {
                host: &self.host,
                user: &self.user,
                password: &self.password,
                hostname: &self.hostname,
                manage_hostname: &self.manage_hostname,
                os_root_password: &self.os_root_password,
                os_root_password_confirmation: &self.os_root_password_confirmation,
                os_pi_password: &self.os_pi_password,
                os_pi_password_confirmation: &self.os_pi_password_confirmation,
                wifi_ssid: &self.wifi_ssid,
                wifi_password: &self.wifi_password,
                wifi_connect: self.wifi_connect,
                wifi_preserve_existing_profiles: self.wifi_preserve_existing_profiles,
                rotate_web_password: self.rotate_web_password,
                web_password: &self.web_password,
                web_password_confirmation: &self.web_password_confirmation,
            }
        }

        fn connection(&self) -> Result<Connection, String> {
            let password =
                Secret::new((*self.password).clone()).map_err(|error| error.to_string())?;
            let sudo_password = if self.sudo_password.is_empty() {
                None
            } else {
                Some(
                    Secret::new((*self.sudo_password).clone())
                        .map_err(|error| error.to_string())?,
                )
            };
            // Alpine's root password is the bootstrap secret: first Deploy
            // uses it once through `su` to install bash/sudo when the SSH
            // account is not root. Connection no longer asks for it separately.
            let bootstrap_root_password = if self.os_root_password.is_empty() {
                None
            } else {
                Some(
                    Secret::new((*self.os_root_password).clone())
                        .map_err(|error| error.to_string())?,
                )
            };
            let connection = Connection {
                host: self.host.clone(),
                username: self.user.clone(),
                port: 22,
                auth: AuthMethod::Password,
                password: Some(password),
                key_path: None,
                key_passphrase: None,
                known_hosts_path: if self.known_hosts.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(&self.known_hosts))
                },
                sudo_password,
                bootstrap_root_password,
            };
            omt_deployer_core::validate_connection(&connection)
                .map_err(|error| error.to_string())?;
            Ok(connection)
        }

        fn start_job(&mut self, job: Job) {
            if self.running() {
                return;
            }
            let connection = match self.connection() {
                Ok(value) => Some(value),
                Err(error) => {
                    self.activity.push(error);
                    self.view = View::Activity;
                    return;
                }
            };
            let changes_web_password = matches!(job, Job::WebPassword)
                || (matches!(job, Job::Deploy) && self.rotate_web_password);
            self.apply_alpine_login = matches!(job, Job::Alpine);
            let request = JobRequest {
                job,
                connection,
                options: self.deploy_options(),
                wifi_ssid: self.wifi_ssid.clone(),
                wifi_password: self.wifi_password.clone(),
                wifi_connect: self.wifi_connect,
                wifi_preserve_existing_profiles: self.wifi_preserve_existing_profiles,
                hostname: self.hostname.clone(),
                manage_hostname: self.manage_hostname.clone(),
                os_root_password: self.os_root_password.clone(),
                os_pi_password: self.os_pi_password.clone(),
                rotate_web_password: self.rotate_web_password,
                web_password: self.web_password.clone(),
            };
            if changes_web_password {
                self.web_password.clear();
                self.web_password_confirmation.clear();
            }
            let (tx, rx) = mpsc::channel();
            self.cancel.store(false, Ordering::Relaxed);
            let cancel = Arc::clone(&self.cancel);
            self.events = Some(rx);
            self.view = View::Activity;
            self.activity.push(match request.job {
                Job::Test => "Testing connection...".into(),
                Job::Alpine => "Starting Alpine sys-mode install...".into(),
                Job::Deploy => "Starting deployment...".into(),
                Job::Manage(ManagementAction::Status) => "Fetching status...".into(),
                Job::Manage(ManagementAction::Logs) => "Fetching logs...".into(),
                Job::Manage(ManagementAction::Restart) => "Restarting service...".into(),
                Job::Manage(ManagementAction::Reboot) => {
                    "Scheduling operating-system reboot...".into()
                }
                Job::WebPassword => "Changing Web GUI password...".into(),
                Job::Hostname => "Renaming the appliance...".into(),
                Job::Wifi => "Applying Wi-Fi settings...".into(),
            });
            thread::spawn(move || {
                let result = run_job(request, &cancel, &tx);
                let _ = tx.send(WorkerEvent::Finished(result));
            });
        }

        /// Open a native folder or file dialog for `target`.
        ///
        /// On its own thread: a modal dialog blocks until the operator answers
        /// it, and doing that inside `update` freezes the window behind it --
        /// including the Cancel button of anything already running.
        fn start_picker(&mut self, target: Picking) {
            if self.picker.is_some() {
                return;
            }
            let start = match target {
                Picking::KnownHosts => self.known_hosts.clone(),
            };
            let (tx, rx) = mpsc::channel();
            self.picker = Some((target, rx));
            thread::spawn(move || {
                let mut dialog = rfd::FileDialog::new();
                // An existing entry is where the dialog opens. A path that has
                // since been renamed is ignored rather than refused.
                let directory = Path::new(&start);
                let opening = if directory.is_dir() {
                    Some(directory.to_path_buf())
                } else {
                    directory
                        .parent()
                        .filter(|path| path.is_dir())
                        .map(Path::to_path_buf)
                };
                if let Some(opening) = opening {
                    dialog = dialog.set_directory(opening);
                }
                let picked = match target {
                    Picking::KnownHosts => dialog
                        .set_title("Select an OpenSSH known_hosts file")
                        .pick_file(),
                };
                let _ = tx.send(picked);
            });
        }

        /// Apply a finished dialog's answer, if it has one.
        fn poll_picker(&mut self, context: &egui::Context) {
            let Some((target, results)) = self.picker.as_ref() else {
                return;
            };
            let target = *target;
            match results.try_recv() {
                Ok(picked) => {
                    self.picker = None;
                    if let Some(path) = picked {
                        let chosen = path.display().to_string();
                        match target {
                            Picking::KnownHosts => self.known_hosts = chosen,
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    context.request_repaint_after(std::time::Duration::from_millis(100));
                }
                // The dialog thread died without answering: drop it rather than
                // leaving the Browse buttons disabled for the session.
                Err(mpsc::TryRecvError::Disconnected) => self.picker = None,
            }
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
                    Ok(()) => {
                        self.activity.push("Operation completed.".into());
                        if self.apply_alpine_login {
                            self.user = "pi".into();
                            self.password = self.os_pi_password.clone();
                            self.sudo_password = self.os_pi_password.clone();
                            self.activity.push(
                                "Connection updated to user pi. Deploy next; the Alpine root password installs sudo on first deploy."
                                    .into(),
                            );
                        }
                    }
                    Err(error) => self.activity.push(format!("ERROR: {error}")),
                }
                self.apply_alpine_login = false;
                self.events = None;
                self.cancel.store(false, Ordering::Relaxed);
            } else if self.running() {
                context.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        /// Bring the opening window inside the display it landed on.
        ///
        /// eframe clamps the requested size against the *largest* monitor,
        /// with no margin, which is both the wrong monitor on a mixed-DPI desk
        /// and the wrong size under a task bar. `monitor_size` is the only
        /// figure here that describes the display the window is actually on --
        /// egui-winit reads it from `current_monitor` and divides by the
        /// window's pixels-per-point, so it is in the same points as
        /// `screen_rect` whatever the display scaling. That is also why this
        /// works on Windows per-monitor DPI: the size is already in the
        /// window's own point space.
        ///
        /// The fit repeats until the window is observed to fit rather than
        /// settling on the first request, because a resize is a round trip:
        /// the frames immediately after it still report the old size, and a
        /// request the compositor drops would otherwise leave the window
        /// oversized for the whole session. It must not run after the window
        /// has been moved. Wayland often names a monitor only once the surface
        /// is on an output, which can be the frame the operator is already
        /// dragging onto another display; `InnerSize` in the middle of that
        /// drag is what snaps the window back. Once it fits, or the budget
        /// expires, this is done for good.
        fn fit_window(&mut self, context: &egui::Context) {
            let Fit::Pending {
                mut started,
                mut origin,
            } = self.fit
            else {
                return;
            };
            if let Some(outer) = context.input(|input| input.viewport().outer_rect) {
                if let Some(first) = origin {
                    if layout::origin_moved([first.x, first.y], [outer.min.x, outer.min.y]) {
                        self.fit = Fit::Settled;
                        return;
                    }
                } else {
                    origin = Some(outer.min);
                }
            }
            // A backend that has not named a monitor yet leaves the fit
            // pending rather than spending the budget on a guess. On Wayland
            // this is every frame before the surface is on an output. The
            // budget starts on the first named-monitor frame, not at process
            // start, so a slow map still gets a fit.
            let Some(monitor) = context.input(|input| input.viewport().monitor_size) else {
                self.fit = Fit::Pending { started, origin };
                return;
            };
            let started_at = started.unwrap_or_else(std::time::Instant::now);
            started = Some(started_at);
            let current = context.screen_rect().size();
            match layout::fit_step(
                [current.x, current.y],
                [monitor.x, monitor.y],
                started_at.elapsed(),
            ) {
                layout::FitStep::Done => {
                    self.fit = Fit::Settled;
                }
                layout::FitStep::Shrink(fitted) => {
                    self.fit = Fit::Pending { started, origin };
                    context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        fitted[0], fitted[1],
                    )));
                    // The resize arrives asynchronously; without this the next
                    // frame may not come until some input does, and the fit
                    // would stall half-applied. Termination is the wall-clock
                    // budget in `fit_step`, so a tight repaint loop cannot
                    // spend the opening on frames that all still show the old
                    // size.
                    context.request_repaint();
                }
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
                ui.label(if self.running() { "Busy" } else { "Idle" });
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

    /// A path field with a dialog button beside it. True when it was clicked.
    ///
    /// Laid out right to left so the button takes the width it needs and the
    /// field fills whatever is left. Placing the field first and asking for
    /// `f32::INFINITY` claims the whole row and pushes the button out of a
    /// narrow window, which is the size this application is expected to run at.
    fn path_field(ui: &mut egui::Ui, text: &mut String, enabled: bool) -> bool {
        let mut browse = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            browse = ui
                .add_enabled(enabled, egui::Button::new("Browse..."))
                .on_hover_text("Choose this on disk instead of typing the path")
                .clicked();
            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        });
        browse
    }

    impl eframe::App for App {
        fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
            self.poll_worker(context);
            self.poll_picker(context);
            self.fit_window(context);
            Self::zoom_input(context);
            egui::TopBottomPanel::top("navigation").show(context, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (name, view) in [
                        ("Connection", View::Connection),
                        ("Alpine", View::Alpine),
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
                View::Connection => {
                    let mut browse = false;
                    let mut test = false;
                    column(ui, |ui| {
                        ui.heading("Connection");
                        field(ui, "Pi host", |ui| text_field(ui, &mut self.host, false));
                        field(ui, "SSH username", |ui| {
                            text_field(ui, &mut self.user, false);
                        });
                        field(ui, "SSH password", |ui| {
                            text_field(ui, &mut self.password, !self.reveal);
                        });
                        ui.label(
                            egui::RichText::new(
                                "Factory images: user root and an empty password, then the Alpine view. \
                                 After Alpine setup this app switches to pi.",
                            )
                            .italics(),
                        );
                        field(ui, "sudo password (optional)", |ui| {
                            text_field(ui, &mut self.sudo_password, !self.reveal);
                        });
                        let idle = !self.running() && self.picker.is_none();
                        field(ui, "known_hosts (optional)", |ui| {
                            browse = path_field(ui, &mut self.known_hosts, idle);
                        });
                        ui.checkbox(&mut self.reveal, "Reveal secrets");
                        ui.add_space(ui.spacing().item_spacing.y);
                        test = ui
                            .add_enabled(
                                self.form().can_connect() && !self.running(),
                                egui::Button::new("Test connection"),
                            )
                            .clicked();
                    });
                    if browse {
                        self.start_picker(Picking::KnownHosts);
                    }
                    if test {
                        self.start_job(Job::Test);
                    }
                }
                View::Alpine => {
                    let mut start = false;
                    column(ui, |ui| {
                        ui.heading("Alpine");
                        ui.label(
                            "Install Alpine in persistent sys mode on a factory Raspberry Pi image. \
                             This erases the SD/eMMC/USB disk. IPv4 uses DHCP on Ethernet, and on \
                             Wi-Fi when an SSID is set or the image already has Wi-Fi. The pi user \
                             is created in the wheel group. First Deploy uses the root password \
                             below to install bash and sudo when you connect as pi.",
                        );
                        ui.add_space(ui.spacing().item_spacing.y);
                        field(ui, "Hostname", |ui| {
                            text_field(ui, &mut self.hostname, false);
                        });
                        field(ui, "Wi-Fi SSID (optional; blank keeps image Wi-Fi)", |ui| {
                            text_field(ui, &mut self.wifi_ssid, false);
                        });
                        field(ui, "Wi-Fi password", |ui| {
                            text_field(ui, &mut self.wifi_password, !self.reveal);
                        });
                        field(ui, "Root password", |ui| {
                            text_field(ui, &mut self.os_root_password, !self.reveal);
                        });
                        field(ui, "Confirm root password", |ui| {
                            text_field(ui, &mut self.os_root_password_confirmation, !self.reveal);
                        });
                        field(ui, "pi password", |ui| {
                            text_field(ui, &mut self.os_pi_password, !self.reveal);
                        });
                        field(ui, "Confirm pi password", |ui| {
                            text_field(ui, &mut self.os_pi_password_confirmation, !self.reveal);
                        });
                        ui.checkbox(&mut self.reveal, "Reveal secrets");
                        ui.add_space(ui.spacing().item_spacing.y);
                        if self.pending_alpine_confirm {
                            ui.label(
                                egui::RichText::new(
                                    "Erase the boot disk and install Alpine in persistent sys mode? \
                                     The Pi will reboot when it finishes.",
                                )
                                .strong(),
                            );
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .add_enabled(
                                        self.form().can_install_alpine() && !self.running(),
                                        egui::Button::new("Confirm Alpine install"),
                                    )
                                    .clicked()
                                {
                                    self.pending_alpine_confirm = false;
                                    start = true;
                                }
                                if ui
                                    .add_enabled(!self.running(), egui::Button::new("Cancel"))
                                    .clicked()
                                {
                                    self.pending_alpine_confirm = false;
                                }
                            });
                        } else if ui
                            .add_enabled(
                                self.form().can_install_alpine() && !self.running(),
                                egui::Button::new("Install Alpine (sys mode)"),
                            )
                            .clicked()
                        {
                            self.pending_alpine_confirm = true;
                        }
                    });
                    if start {
                        self.start_job(Job::Alpine);
                    }
                }
                View::Deploy => {
                    let mut deploy = false;
                    column(ui, |ui| {
                        ui.heading("Deploy");
                        ui.label(
                            "Upload, verify, recover, and promote the manifest-v3 capsule this \
                             deployer carries.",
                        );
                        ui.label(egui::RichText::new(Self::capsule_line()).italics());
                        ui.add_space(ui.spacing().item_spacing.y);
                        field(ui, "Remote directory", |ui| {
                            text_field(ui, &mut self.remote_directory, false);
                        });
                        ui.checkbox(
                            &mut self.rotate_web_password,
                            "Rotate the Web GUI password after deploy",
                        )
                        .on_hover_text(
                            "Off by default. When enabled, the password entered below replaces \
                             the appliance credential after install and signs out every Web session.",
                        );
                        if self.rotate_web_password {
                            ui.label(
                                "Set a 12-128 byte password. Changing it restarts the appliance.",
                            );
                            field(ui, "New Web GUI password", |ui| {
                                text_field(ui, &mut self.web_password, !self.reveal);
                            });
                            field(ui, "Confirm password", |ui| {
                                text_field(ui, &mut self.web_password_confirmation, !self.reveal);
                            });
                            ui.checkbox(&mut self.reveal, "Reveal secrets");
                        }
                        ui.add_space(ui.spacing().item_spacing.y);
                        deploy = ui
                            .add_enabled(
                                self.form().can_deploy() && !self.running(),
                                egui::Button::new("Deploy"),
                            )
                            .clicked();
                    });
                    if deploy {
                        self.start_job(Job::Deploy);
                    }
                }
                View::Manage => column(ui, |ui| {
                    ui.heading("Manage");
                    // Wrapped, so the controls remain reachable at the minimum
                    // supported window width.
                    ui.horizontal_wrapped(|ui| {
                        for (label, action) in [
                            ("Status", ManagementAction::Status),
                            ("Logs", ManagementAction::Logs),
                            ("Restart", ManagementAction::Restart),
                        ] {
                            if ui
                                .add_enabled(!self.running(), egui::Button::new(label))
                                .clicked()
                            {
                                self.start_job(Job::Manage(action));
                            }
                        }
                        if ui
                            .add_enabled(!self.running(), egui::Button::new("Reboot OS"))
                            .clicked()
                        {
                            self.pending_confirmation = Some(ManagementAction::Reboot);
                        }
                    });
                    if self.pending_confirmation == Some(ManagementAction::Reboot) {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "Reboot the Raspberry Pi? The web UI and playback will be unavailable during boot.",
                            )
                            .strong(),
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(
                                    !self.running(),
                                    egui::Button::new("Confirm reboot"),
                                )
                                .clicked()
                            {
                                self.pending_confirmation = None;
                                self.start_job(Job::Manage(ManagementAction::Reboot));
                            }
                            if ui
                                .add_enabled(!self.running(), egui::Button::new("Cancel"))
                                .clicked()
                            {
                                self.pending_confirmation = None;
                            }
                        });
                    }
                    ui.separator();
                    ui.heading("Appliance hostname");
                    ui.label(
                        "The name shown in the Web GUI and published as <name>.local. Applying it \
                         recreates the appliance container, so playback stops for a few seconds.",
                    );
                    field(ui, "New hostname", |ui| {
                        text_field(ui, &mut self.manage_hostname, false);
                    });
                    if ui
                        .add_enabled(
                            self.form().can_set_hostname() && !self.running(),
                            egui::Button::new("Change hostname"),
                        )
                        .clicked()
                    {
                        self.start_job(Job::Hostname);
                    }
                    ui.separator();
                    ui.heading("Web GUI password");
                    ui.label(
                        "Optional. Set a 12-128 byte password. Changing it restarts the appliance and signs out every Web session.",
                    );
                    field(ui, "New password", |ui| {
                        text_field(ui, &mut self.web_password, !self.reveal);
                    });
                    field(ui, "Confirm password", |ui| {
                        text_field(ui, &mut self.web_password_confirmation, !self.reveal);
                    });
                    ui.checkbox(&mut self.reveal, "Reveal secrets");
                    if ui
                        .add_enabled(
                            self.form().can_change_web_password() && !self.running(),
                            egui::Button::new("Change Web GUI password"),
                        )
                        .clicked()
                    {
                        self.start_job(Job::WebPassword);
                    }
                }),
                View::Wifi => column(ui, |ui| {
                    ui.heading("Wi-Fi");
                    field(ui, "SSID", |ui| text_field(ui, &mut self.wifi_ssid, false));
                    field(ui, "Passphrase", |ui| {
                        text_field(ui, &mut self.wifi_password, !self.reveal);
                    });
                    ui.checkbox(
                        &mut self.wifi_preserve_existing_profiles,
                        "Preserve existing Wi-Fi profiles",
                    )
                    .on_hover_text(
                        "Uncheck to remove every other saved network; connection is verified first when requested",
                    );
                    ui.checkbox(&mut self.wifi_connect, "Connect immediately after saving")
                        .on_hover_text(
                            "Uncheck to save the profile for a device that will connect at a new location",
                        );
                    ui.add_space(ui.spacing().item_spacing.y);
                    let enabled = self.form().can_apply_wifi() && !self.running();
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
                        if self.running() && ui.button("Cancel").clicked() {
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
                    ui.label(Self::capsule_line());
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
                // compositors. It clamps against the largest monitor, which is
                // only a crash guard; `App::fit_window` does the real fit
                // against the monitor the window actually opened on.
                .with_clamp_size_to_monitor_size(true),
            // Deliberately not `centered`, and not the egui helper that
            // centres the viewport on the primary monitor. Both take that
            // monitor's size and use half of it as an absolute desktop
            // position, never adding that monitor's origin. On a single
            // display whose origin is (0, 0) it happens to work. With
            // several, the offset lands wherever it falls in the desktop
            // rectangle -- routinely a different display than the primary --
            // and mixed scale factors move it again, because the value is
            // then converted to physical pixels with one monitor's factor.
            // egui exposes no monitor origin to correct this with, and this
            // crate adds no extra windowing dependency to read one.
            //
            // Leaving the position unset is the placement that works on every
            // desktop this binary ships for. Windows then uses CW_USEDEFAULT,
            // which puts the window on the cursor's display at that display's
            // scale. Linux and macOS window managers place new windows on the
            // active display and inside its work area. `App::fit_window` then
            // shrinks to `current_monitor` in that window's own points, so a
            // 200%-scaled panel is measured in the same space as the window.
            centered: false,
            ..eframe::NativeOptions::default()
        };
        eframe::run_native(
            "Raspberry Pi OMT Deployer",
            options,
            Box::new(|_| Ok(Box::<App>::default())),
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn connection_keeps_ssh_and_sudo_credentials_separate() {
            let app = App {
                user: "pi".into(),
                password: Zeroizing::new("ssh-password".into()),
                sudo_password: Zeroizing::new("sudo-password".into()),
                os_root_password: Zeroizing::new("root-password".into()),
                ..App::default()
            };

            let connection = app.connection().unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                connection.password.as_ref().map(Secret::expose),
                Some("ssh-password")
            );
            assert_eq!(
                connection.sudo_password.as_ref().map(Secret::expose),
                Some("sudo-password")
            );
            assert_eq!(
                connection
                    .bootstrap_root_password
                    .as_ref()
                    .map(Secret::expose),
                Some("root-password")
            );
        }

        #[test]
        fn alpine_root_password_is_the_bootstrap_secret() {
            let without_root = App {
                user: "pi".into(),
                password: Zeroizing::new("ssh-password".into()),
                ..App::default()
            };
            assert!(
                without_root
                    .connection()
                    .unwrap_or_else(|error| panic!("{error}"))
                    .bootstrap_root_password
                    .is_none()
            );
        }

        #[test]
        fn connection_does_not_ask_for_a_separate_bootstrap_root_password() {
            let source = include_str!("main.rs");
            assert!(
                !source.contains(&["initial", "root", "password"].join(" ")),
                "factory bootstrap belongs on the Alpine view"
            );
            assert!(!source.contains(&["clean", "Alpine", "only"].join(" ")));
        }

        /// eframe's centred-at-init flag and egui's centre-on-primary-monitor
        /// helper both treat half the primary monitor's size as an absolute
        /// desktop position, which is how a mixed-DPI Windows desk opens the
        /// window on the wrong display. Placement is leaving the position unset.
        #[test]
        fn the_window_is_not_centred_on_the_primary_monitor() {
            let source = include_str!("main.rs");
            assert!(source.contains("centered: false"));
            assert!(
                !source.contains(&["centered:", "true"].join(" ")),
                "eframe centred placement ignores monitor origin on Windows"
            );
        }

        /// `running` used to be a field kept beside the channel it described,
        /// which is one state with two ways to be wrong. It is derived now.
        #[test]
        fn busy_follows_the_state_it_describes() {
            let mut app = App::default();
            assert!(!app.running());

            let (_tx, rx) = mpsc::channel();
            app.events = Some(rx);
            assert!(app.running());
            app.events = None;
            assert!(!app.running());
        }

        /// The deployment this application performs is the capsule compiled
        /// into it: no project root to point at, and nothing to build. A
        /// `project_root` reaching the core from here would mean the GUI had
        /// grown a source-tree dependency again.
        #[test]
        fn the_gui_deploys_only_the_embedded_capsule() {
            let options = App::default().deploy_options();
            assert!(options.project_root.is_none());
            assert!(!options.rebuild_image);
            assert_eq!(options.remote_directory, "/opt/omt-client");
        }

        /// The archive is the appliance, and a deployer built without one
        /// could not deploy at all. `build.rs` refuses to produce that, and
        /// this is the same claim checked against the linked binary.
        #[test]
        fn the_appliance_image_is_inside_this_binary() {
            let image = embedded_image().unwrap_or_else(|| panic!("no embedded image"));
            assert_eq!(image.name, IMAGE_MEMBER);
            assert!(image.bytes.len() > 1024 * 1024);
            let line = App::capsule_line();
            assert!(line.contains(IMAGE_MEMBER), "{line}");
            assert!(line.contains("MiB"), "{line}");
        }

        #[test]
        fn alpine_defaults_to_omt_client_hostname() {
            assert_eq!(App::default().hostname, "omt-client");
            assert!(!App::default().pending_alpine_confirm);
        }

        #[test]
        fn web_password_rotation_is_off_by_default() {
            assert!(!App::default().rotate_web_password);
        }

        #[test]
        fn wifi_defaults_preserve_profiles_and_connect_immediately() {
            let app = App::default();
            assert!(app.wifi_preserve_existing_profiles);
            assert!(app.wifi_connect);
        }
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
