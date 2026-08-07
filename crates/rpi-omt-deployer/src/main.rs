// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]

#[cfg(feature = "desktop")]
mod desktop {
    use eframe::egui;
    use omt_deployer_core::{valid_host, valid_username};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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
    pub struct App {
        view: View,
        host: String,
        user: String,
        password: Zeroizing<String>,
        wifi_password: Zeroizing<String>,
        reveal: bool,
        activity: Vec<String>,
        cancel: Arc<AtomicBool>,
        running: bool,
    }
    impl Default for App {
        fn default() -> Self {
            Self {
                view: View::Connection,
                host: "raspberrypi.local".into(),
                user: "root".into(),
                password: Zeroizing::new(String::new()),
                wifi_password: Zeroizing::new(String::new()),
                reveal: false,
                activity: Vec::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                running: false,
            }
        }
    }
    impl eframe::App for App {
        fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
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
                    ui.add_enabled(valid && !self.running, egui::Button::new("Test connection"));
                }
                View::Deploy => {
                    ui.heading("Deploy");
                    ui.label(
                        "Build, verify, upload, recover, and promote the manifest-v3 capsule.",
                    );
                    ui.add_enabled(
                        valid_host(&self.host) && !self.running,
                        egui::Button::new("Deploy"),
                    );
                }
                View::Manage => {
                    ui.heading("Manage");
                    ui.horizontal(|ui| {
                        for action in ["Status", "Logs", "Restart"] {
                            ui.add_enabled(!self.running, egui::Button::new(action));
                        }
                    });
                }
                View::Wifi => {
                    ui.heading("Wi-Fi");
                    ui.label("Passphrase");
                    ui.add(
                        egui::TextEdit::singleline(&mut *self.wifi_password).password(!self.reveal),
                    );
                    ui.add_enabled(
                        (8..=64).contains(&self.wifi_password.len()) && !self.running,
                        egui::Button::new("Apply Wi-Fi"),
                    );
                }
                View::Activity => {
                    ui.heading("Activity");
                    if self.running && ui.button("Cancel").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
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
        eframe::run_native(
            "Raspberry Pi OMT Deployer",
            eframe::NativeOptions::default(),
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
