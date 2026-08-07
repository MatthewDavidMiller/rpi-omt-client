// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]
#![cfg_attr(feature = "desktop", windows_subsystem = "windows")]

#[cfg(feature = "desktop")]
mod desktop {
    use eframe::egui;
    use omt_deployer_core::{
        AuthMethod, Connection, DeployOptions, ManagementAction, Secret, WifiSettings, apply_wifi,
        deploy, discover_project_root, manage, test_connection, valid_host, valid_username,
        validate_wifi,
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
        last_password: Zeroizing<String>,
        last_wifi_password: Zeroizing<String>,
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
                last_password: Zeroizing::new(String::new()),
                last_wifi_password: Zeroizing::new(String::new()),
            }
        }
    }

    impl App {
        fn wipe_replaced_secrets(&mut self) {
            if *self.password != *self.last_password {
                self.last_password = Zeroizing::new((*self.password).clone());
            }
            if *self.wifi_password != *self.last_wifi_password {
                self.last_wifi_password = Zeroizing::new((*self.wifi_password).clone());
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
                    image_name: "omt-client".into(),
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
            self.wipe_replaced_secrets();
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
                    let valid = valid_host(&self.host)
                        && valid_username(&self.user)
                        && !self.password.is_empty();
                    if ui
                        .add_enabled(valid && !self.running, egui::Button::new("Test connection"))
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
                    if ui
                        .add_enabled(
                            valid_host(&self.host)
                                && !self.project_root.is_empty()
                                && !self.running,
                            egui::Button::new("Deploy"),
                        )
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
                    if ui
                        .add_enabled(
                            !self.wifi_ssid.is_empty()
                                && (8..=64).contains(&self.wifi_password.len())
                                && !self.running,
                            egui::Button::new("Apply Wi-Fi"),
                        )
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
