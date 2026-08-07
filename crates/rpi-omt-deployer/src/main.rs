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

#[cfg(feature = "desktop")]
mod desktop {
    use crate::gates::Form;
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
        activity: Vec<String>,
        cancel: Arc<AtomicBool>,
        running: bool,
        events: Option<Receiver<WorkerEvent>>,
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
                activity: Vec::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                running: false,
                events: None,
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
                wifi_password: (*self.wifi_password).clone(),
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
    }

    struct JobRequest {
        job: Job,
        connection: Connection,
        project_root: PathBuf,
        remote_directory: String,
        wifi_ssid: String,
        wifi_password: String,
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
                    password: Secret::new(request.wifi_password)
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
            egui::CentralPanel::default().show(context, |ui| match self.view {
                View::Connection => {
                    ui.heading("Connection");
                    ui.label("Pi host");
                    ui.text_edit_singleline(&mut self.host);
                    ui.label("SSH username");
                    ui.text_edit_singleline(&mut self.user);
                    ui.label("SSH password");
                    ui.add(egui::TextEdit::singleline(&mut *self.password).password(!self.reveal));
                    ui.checkbox(&mut self.reveal, "Reveal secrets");
                    let enabled = self.form().can_connect() && !self.running;
                    if ui
                        .add_enabled(enabled, egui::Button::new("Test connection"))
                        .clicked()
                    {
                        self.start_job(Job::Test);
                    }
                }
                View::Deploy => {
                    ui.heading("Deploy");
                    ui.label(
                        "Build, verify, upload, recover, and promote the manifest-v3 capsule.",
                    );
                    ui.label("Project root");
                    ui.text_edit_singleline(&mut self.project_root);
                    ui.label("Remote directory");
                    ui.text_edit_singleline(&mut self.remote_directory);
                    let enabled = self.form().can_deploy() && !self.running;
                    if ui
                        .add_enabled(enabled, egui::Button::new("Deploy"))
                        .clicked()
                    {
                        self.start_job(Job::Deploy);
                    }
                }
                View::Manage => {
                    ui.heading("Manage");
                    ui.horizontal(|ui| {
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
                }
                View::Wifi => {
                    ui.heading("Wi-Fi");
                    ui.label("SSID");
                    ui.text_edit_singleline(&mut self.wifi_ssid);
                    ui.label("Passphrase");
                    ui.add(
                        egui::TextEdit::singleline(&mut *self.wifi_password).password(!self.reveal),
                    );
                    ui.checkbox(&mut self.wifi_connect, "Connect after saving");
                    let enabled = self.form().can_apply_wifi() && !self.running;
                    if ui
                        .add_enabled(enabled, egui::Button::new("Apply Wi-Fi"))
                        .clicked()
                    {
                        self.start_job(Job::Wifi);
                    }
                }
                View::Activity => {
                    ui.heading("Activity");
                    if self.running && ui.button("Cancel").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                        self.activity.push("Cancellation requested...".into());
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for line in &self.activity {
                            ui.label(line);
                        }
                    });
                }
                View::About => {
                    ui.heading("Raspberry Pi OMT Deployer");
                    ui.label(VERSION);
                    egui::ScrollArea::vertical().show(ui, |ui| {
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
                .with_inner_size([960.0, 640.0])
                .with_min_inner_size([720.0, 480.0]),
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
