// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//! State and input handling for the terminal deployer.
//!
//! The field set mirrors the egui application's exactly, so `docs/SETUP.md`
//! describes one deployer rather than two, and an operator moving between a
//! Windows workstation and a Linux one is filling in the same form.
//!
//! Rendering lives in `ui`; this module never draws.

use omt_deployer_core::{
    AuthMethod, Connection, DeployOptions, Job, JobRequest, ManagementAction, Secret, WorkerEvent,
    run_job, valid_appliance_hostname, validate_connection,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use zeroize::Zeroizing;

/// How many progress lines the activity log keeps.
///
/// A deployment that uploads an appliance image emits far more than a screen's
/// worth, and an operator scrolling back for the failure wants the recent end.
const LOG_CAPACITY: usize = 2000;

pub const VERSION: &str = match option_env!("RPI_OMT_CLIENT_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Connection,
    Alpine,
    Deploy,
    Manage,
    Wifi,
    Activity,
    About,
}

impl View {
    pub const ALL: [Self; 7] = [
        Self::Connection,
        Self::Alpine,
        Self::Deploy,
        Self::Manage,
        Self::Wifi,
        Self::Activity,
        Self::About,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Connection => "Connection",
            Self::Alpine => "Alpine",
            Self::Deploy => "Deploy",
            Self::Manage => "Manage",
            Self::Wifi => "Wi-Fi",
            Self::Activity => "Activity",
            Self::About => "About",
        }
    }

    /// The focusable rows this view shows, in tab order.
    pub fn slots(self) -> &'static [Slot] {
        match self {
            Self::Connection => &[
                Slot::Host,
                Slot::User,
                Slot::Password,
                Slot::SudoPassword,
                Slot::KnownHosts,
                Slot::TestButton,
            ],
            Self::Alpine => &[
                Slot::AlpineHostname,
                Slot::AlpineRootPassword,
                Slot::AlpineRootConfirm,
                Slot::AlpinePiPassword,
                Slot::AlpinePiConfirm,
                Slot::AlpineWifiSsid,
                Slot::AlpineWifiPassword,
                Slot::AlpineApplyLogin,
                Slot::AlpineButton,
            ],
            Self::Deploy => &[
                Slot::RemoteDirectory,
                Slot::RotateWebPassword,
                Slot::WebPassword,
                Slot::WebConfirm,
                Slot::DeployButton,
            ],
            Self::Manage => &[
                Slot::StatusButton,
                Slot::LogsButton,
                Slot::RestartButton,
                Slot::RebootButton,
                Slot::ManageHostname,
                Slot::HostnameButton,
                Slot::ManageWebPassword,
                Slot::ManageWebConfirm,
                Slot::WebPasswordButton,
            ],
            Self::Wifi => &[
                Slot::WifiSsid,
                Slot::WifiPassword,
                Slot::WifiConnect,
                Slot::WifiPreserve,
                Slot::WifiButton,
            ],
            Self::Activity | Self::About => &[],
        }
    }
}

/// One focusable row. Flat across every view so focus is a single index and
/// the render and input paths cannot disagree about what is selected.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Host,
    User,
    Password,
    SudoPassword,
    KnownHosts,
    TestButton,

    AlpineHostname,
    AlpineRootPassword,
    AlpineRootConfirm,
    AlpinePiPassword,
    AlpinePiConfirm,
    AlpineWifiSsid,
    AlpineWifiPassword,
    AlpineApplyLogin,
    AlpineButton,

    RemoteDirectory,
    RotateWebPassword,
    WebPassword,
    WebConfirm,
    DeployButton,

    StatusButton,
    LogsButton,
    RestartButton,
    RebootButton,
    ManageHostname,
    HostnameButton,
    ManageWebPassword,
    ManageWebConfirm,
    WebPasswordButton,

    WifiSsid,
    WifiPassword,
    WifiConnect,
    WifiPreserve,
    WifiButton,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Secret,
    Toggle,
    Button,
}

impl Slot {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Host => "Host",
            Self::User => "Username",
            Self::Password => "SSH password",
            Self::SudoPassword => "sudo password (optional)",
            Self::KnownHosts => "known_hosts path (optional)",
            Self::TestButton => "Test connection",

            Self::AlpineHostname => "Appliance hostname",
            Self::AlpineRootPassword => "New root password",
            Self::AlpineRootConfirm => "Confirm root password",
            Self::AlpinePiPassword => "New pi password",
            Self::AlpinePiConfirm => "Confirm pi password",
            Self::AlpineWifiSsid => "Wi-Fi SSID (optional)",
            Self::AlpineWifiPassword | Self::WifiPassword => "Wi-Fi password",
            Self::AlpineApplyLogin => "Use the new pi login after setup",
            Self::AlpineButton => "Run Alpine setup",

            Self::RemoteDirectory => "Remote directory",
            Self::RotateWebPassword => "Also set the Web GUI password",
            Self::WebPassword => "Web GUI password",
            Self::WebConfirm | Self::ManageWebConfirm => "Confirm Web GUI password",
            Self::DeployButton => "Deploy",

            Self::StatusButton => "Status",
            Self::LogsButton => "Logs",
            Self::RestartButton => "Restart the appliance",
            Self::RebootButton => "Reboot the Raspberry Pi",
            Self::ManageHostname => "Rename appliance to",
            Self::HostnameButton => "Apply hostname",
            Self::ManageWebPassword => "New Web GUI password",
            Self::WebPasswordButton => "Change Web GUI password",

            Self::WifiSsid => "SSID",
            Self::WifiConnect => "Connect now",
            Self::WifiPreserve => "Keep other saved profiles",
            Self::WifiButton => "Apply Wi-Fi settings",
        }
    }

    pub const fn kind(self) -> Kind {
        match self {
            Self::Host
            | Self::User
            | Self::KnownHosts
            | Self::AlpineHostname
            | Self::AlpineWifiSsid
            | Self::RemoteDirectory
            | Self::ManageHostname
            | Self::WifiSsid => Kind::Text,

            Self::Password
            | Self::SudoPassword
            | Self::AlpineRootPassword
            | Self::AlpineRootConfirm
            | Self::AlpinePiPassword
            | Self::AlpinePiConfirm
            | Self::AlpineWifiPassword
            | Self::WebPassword
            | Self::WebConfirm
            | Self::ManageWebPassword
            | Self::ManageWebConfirm
            | Self::WifiPassword => Kind::Secret,

            Self::AlpineApplyLogin
            | Self::RotateWebPassword
            | Self::WifiConnect
            | Self::WifiPreserve => Kind::Toggle,

            Self::TestButton
            | Self::AlpineButton
            | Self::DeployButton
            | Self::StatusButton
            | Self::LogsButton
            | Self::RestartButton
            | Self::RebootButton
            | Self::HostnameButton
            | Self::WebPasswordButton
            | Self::WifiButton => Kind::Button,
        }
    }
}

/// A destructive action waiting for a yes.
///
/// Restart and Reboot interrupt a running appliance, so they are confirmed the
/// same way the egui application confirms them rather than firing on a
/// keystroke that could have been a mistyped tab.
pub struct Pending {
    pub action: ManagementAction,
    pub prompt: String,
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub view: View,
    pub focus: usize,
    pub cursor: usize,
    pub reveal: bool,
    pub should_quit: bool,
    pub status: String,

    pub host: String,
    pub user: String,
    pub password: Zeroizing<String>,
    pub sudo_password: Zeroizing<String>,
    pub known_hosts: String,

    pub hostname: String,
    pub os_root_password: Zeroizing<String>,
    pub os_root_confirm: Zeroizing<String>,
    pub os_pi_password: Zeroizing<String>,
    pub os_pi_confirm: Zeroizing<String>,
    pub apply_alpine_login: bool,

    pub remote_directory: String,
    pub rotate_web_password: bool,
    pub web_password: Zeroizing<String>,
    pub web_confirm: Zeroizing<String>,

    /// Kept apart from `hostname` so the Alpine view's factory-image name can
    /// never drive a live rename.
    pub manage_hostname: String,
    pub manage_web_password: Zeroizing<String>,
    pub manage_web_confirm: Zeroizing<String>,

    pub wifi_ssid: String,
    pub wifi_password: Zeroizing<String>,
    pub wifi_connect: bool,
    pub wifi_preserve: bool,

    pub log: Vec<String>,
    pub log_scroll: usize,
    pub follow_log: bool,
    pub pending: Option<Pending>,

    cancel: Arc<AtomicBool>,
    events: Option<Receiver<WorkerEvent>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            view: View::Connection,
            focus: 0,
            cursor: 0,
            reveal: false,
            should_quit: false,
            status: String::from("Ready."),
            host: "raspberrypi.local".into(),
            user: "root".into(),
            password: Zeroizing::new(String::new()),
            sudo_password: Zeroizing::new(String::new()),
            known_hosts: String::new(),
            hostname: "omt-client".into(),
            os_root_password: Zeroizing::new(String::new()),
            os_root_confirm: Zeroizing::new(String::new()),
            os_pi_password: Zeroizing::new(String::new()),
            os_pi_confirm: Zeroizing::new(String::new()),
            apply_alpine_login: false,
            remote_directory: "/opt/omt-client".into(),
            rotate_web_password: false,
            web_password: Zeroizing::new(String::new()),
            web_confirm: Zeroizing::new(String::new()),
            manage_hostname: String::new(),
            manage_web_password: Zeroizing::new(String::new()),
            manage_web_confirm: Zeroizing::new(String::new()),
            wifi_ssid: String::new(),
            wifi_password: Zeroizing::new(String::new()),
            wifi_connect: true,
            wifi_preserve: true,
            log: Vec::new(),
            log_scroll: 0,
            follow_log: true,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            events: None,
        }
    }
}

impl App {
    pub fn busy(&self) -> bool {
        self.events.is_some()
    }

    pub fn selected(&self) -> Option<Slot> {
        self.view.slots().get(self.focus).copied()
    }

    /// The editable text behind a slot, secret or not.
    ///
    /// `Zeroizing<String>` derefs to `String`, so the secret fields hand back
    /// the same handle and the caller does not branch on which it got.
    pub fn value_mut(&mut self, slot: Slot) -> Option<&mut String> {
        Some(match slot {
            Slot::Host => &mut self.host,
            Slot::User => &mut self.user,
            Slot::Password => &mut self.password,
            Slot::SudoPassword => &mut self.sudo_password,
            Slot::KnownHosts => &mut self.known_hosts,
            Slot::AlpineHostname => &mut self.hostname,
            Slot::AlpineRootPassword => &mut self.os_root_password,
            Slot::AlpineRootConfirm => &mut self.os_root_confirm,
            Slot::AlpinePiPassword => &mut self.os_pi_password,
            Slot::AlpinePiConfirm => &mut self.os_pi_confirm,
            Slot::AlpineWifiSsid | Slot::WifiSsid => &mut self.wifi_ssid,
            Slot::AlpineWifiPassword | Slot::WifiPassword => &mut self.wifi_password,
            Slot::RemoteDirectory => &mut self.remote_directory,
            Slot::WebPassword => &mut self.web_password,
            Slot::WebConfirm => &mut self.web_confirm,
            Slot::ManageHostname => &mut self.manage_hostname,
            Slot::ManageWebPassword => &mut self.manage_web_password,
            Slot::ManageWebConfirm => &mut self.manage_web_confirm,
            _ => return None,
        })
    }

    pub fn value(&self, slot: Slot) -> &str {
        match slot {
            Slot::Host => &self.host,
            Slot::User => &self.user,
            Slot::Password => &self.password,
            Slot::SudoPassword => &self.sudo_password,
            Slot::KnownHosts => &self.known_hosts,
            Slot::AlpineHostname => &self.hostname,
            Slot::AlpineRootPassword => &self.os_root_password,
            Slot::AlpineRootConfirm => &self.os_root_confirm,
            Slot::AlpinePiPassword => &self.os_pi_password,
            Slot::AlpinePiConfirm => &self.os_pi_confirm,
            Slot::AlpineWifiSsid | Slot::WifiSsid => &self.wifi_ssid,
            Slot::AlpineWifiPassword | Slot::WifiPassword => &self.wifi_password,
            Slot::RemoteDirectory => &self.remote_directory,
            Slot::WebPassword => &self.web_password,
            Slot::WebConfirm => &self.web_confirm,
            Slot::ManageHostname => &self.manage_hostname,
            Slot::ManageWebPassword => &self.manage_web_password,
            Slot::ManageWebConfirm => &self.manage_web_confirm,
            _ => "",
        }
    }

    pub fn toggle_mut(&mut self, slot: Slot) -> Option<&mut bool> {
        Some(match slot {
            Slot::AlpineApplyLogin => &mut self.apply_alpine_login,
            Slot::RotateWebPassword => &mut self.rotate_web_password,
            Slot::WifiConnect => &mut self.wifi_connect,
            Slot::WifiPreserve => &mut self.wifi_preserve,
            _ => return None,
        })
    }

    pub fn toggle(&self, slot: Slot) -> bool {
        match slot {
            Slot::AlpineApplyLogin => self.apply_alpine_login,
            Slot::RotateWebPassword => self.rotate_web_password,
            Slot::WifiConnect => self.wifi_connect,
            Slot::WifiPreserve => self.wifi_preserve,
            _ => false,
        }
    }

    pub fn select_view(&mut self, view: View) {
        self.view = view;
        self.focus = 0;
        self.cursor = 0;
    }

    pub fn move_focus(&mut self, delta: isize) {
        let count = self.view.slots().len();
        if count == 0 {
            return;
        }
        let count_i = isize::try_from(count).unwrap_or(isize::MAX);
        let current = isize::try_from(self.focus).unwrap_or(0);
        let next = (current + delta).rem_euclid(count_i);
        self.focus = usize::try_from(next).unwrap_or(0);
        // Land at the end of the newly focused text so typing continues it
        // rather than inserting at a position left over from another field.
        self.cursor = self
            .selected()
            .map_or(0, |slot| self.value(slot).chars().count());
    }

    pub fn push_log(&mut self, line: String) {
        if self.log.len() >= LOG_CAPACITY {
            self.log.remove(0);
            self.log_scroll = self.log_scroll.saturating_sub(1);
        }
        self.log.push(line);
    }

    /// Drain whatever the worker has produced since the last redraw.
    pub fn poll_worker(&mut self) {
        // Taken out for the drain so the log can be appended to, and put back
        // below unless the job ended.
        let Some(receiver) = self.events.take() else {
            return;
        };
        let mut finished = None;
        let mut lines = Vec::new();
        loop {
            match receiver.try_recv() {
                Ok(WorkerEvent::Line(line)) => lines.push(line),
                Ok(WorkerEvent::Finished(result)) => {
                    finished = Some(result);
                    break;
                }
                // Disconnected without a Finished means the worker thread died
                // without reporting, which is still an end to the job.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = Some(Err("the worker stopped without reporting".into()));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        for line in lines {
            self.push_log(line);
        }
        if finished.is_none() {
            self.events = Some(receiver);
        }
        if let Some(result) = finished {
            self.cancel.store(false, Ordering::SeqCst);
            match result {
                Ok(()) => {
                    self.status = "Finished successfully.".into();
                    self.push_log("-- finished successfully --".into());
                }
                Err(error) => {
                    self.status = format!("Failed: {error}");
                    self.push_log(format!("-- failed: {error} --"));
                }
            }
        }
    }

    pub fn cancel_job(&mut self) {
        if self.busy() {
            self.cancel.store(true, Ordering::SeqCst);
            self.status = "Cancelling...".into();
            self.push_log("-- cancellation requested --".into());
        }
    }

    /// Build the connection the remote jobs share.
    fn connection(&self) -> Result<Connection, String> {
        let password = if self.password.is_empty() {
            None
        } else {
            Some(Secret::new((*self.password).clone()).map_err(|error| error.to_string())?)
        };
        let sudo_password = if self.sudo_password.is_empty() {
            None
        } else {
            Some(Secret::new((*self.sudo_password).clone()).map_err(|error| error.to_string())?)
        };
        let known_hosts_path = if self.known_hosts.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(self.known_hosts.trim()))
        };
        // The Alpine view's root password doubles as the bootstrap secret for
        // a first deployment onto a factory image whose SSH account is not
        // root, matching the CLI's bootstrap_root_password.
        let bootstrap_root_password = if self.os_root_password.is_empty() {
            None
        } else {
            Some(Secret::new((*self.os_root_password).clone()).map_err(|error| error.to_string())?)
        };
        let connection = Connection {
            host: self.host.trim().to_owned(),
            username: self.user.trim().to_owned(),
            port: 22,
            auth: AuthMethod::Password,
            password,
            key_path: None,
            key_passphrase: None,
            known_hosts_path,
            sudo_password,
            bootstrap_root_password,
        };
        validate_connection(&connection).map_err(|error| error.to_string())?;
        Ok(connection)
    }

    /// Reject what the operator can still fix before anything reaches the Pi.
    fn precheck(&self, job: &Job) -> Result<(), String> {
        match job {
            Job::Alpine => {
                if !valid_appliance_hostname(self.hostname.trim()) {
                    return Err(
                        "Appliance hostname must be one DNS label of 1-63 characters".into(),
                    );
                }
                if *self.os_root_password != *self.os_root_confirm {
                    return Err("Root password confirmation does not match".into());
                }
                if *self.os_pi_password != *self.os_pi_confirm {
                    return Err("pi password confirmation does not match".into());
                }
                if !self.wifi_ssid.trim().is_empty() && self.wifi_password.is_empty() {
                    return Err("A Wi-Fi SSID needs a Wi-Fi password".into());
                }
                Ok(())
            }
            Job::Deploy => {
                if self.rotate_web_password && *self.web_password != *self.web_confirm {
                    return Err("Web GUI password confirmation does not match".into());
                }
                Ok(())
            }
            Job::WebPassword => {
                if *self.manage_web_password != *self.manage_web_confirm {
                    return Err("Web GUI password confirmation does not match".into());
                }
                Ok(())
            }
            Job::Hostname => {
                if valid_appliance_hostname(self.manage_hostname.trim()) {
                    Ok(())
                } else {
                    Err("Hostname must be one DNS label of 1-63 characters".into())
                }
            }
            Job::Test | Job::Manage(_) | Job::Wifi => Ok(()),
        }
    }

    fn request(&self, job: Job) -> Result<JobRequest, String> {
        let options = DeployOptions {
            project_root: None,
            remote_directory: self.remote_directory.trim().to_owned(),
            rebuild_image: false,
        };
        // Manage's Web password fields are separate from Deploy's so a
        // rotation typed on one view cannot be submitted from the other.
        let web_password = match job {
            Job::WebPassword => self.manage_web_password.clone(),
            _ => self.web_password.clone(),
        };
        Ok(JobRequest {
            job,
            connection: Some(self.connection()?),
            options,
            wifi_ssid: self.wifi_ssid.trim().to_owned(),
            wifi_password: self.wifi_password.clone(),
            wifi_connect: self.wifi_connect,
            wifi_preserve_existing_profiles: self.wifi_preserve,
            hostname: self.hostname.trim().to_owned(),
            manage_hostname: self.manage_hostname.trim().to_owned(),
            os_root_password: self.os_root_password.clone(),
            os_pi_password: self.os_pi_password.clone(),
            rotate_web_password: self.rotate_web_password,
            web_password,
        })
    }

    pub fn start(&mut self, job: Job) {
        if self.busy() {
            self.status = "A job is already running.".into();
            return;
        }
        if let Err(error) = self.precheck(&job) {
            self.status = format!("Cannot start: {error}");
            return;
        }
        let request = match self.request(job) {
            Ok(request) => request,
            Err(error) => {
                self.status = format!("Cannot start: {error}");
                return;
            }
        };
        let (tx, rx) = channel();
        self.cancel.store(false, Ordering::SeqCst);
        let cancel = Arc::clone(&self.cancel);
        // Detached deliberately: the receiver going away is how a quit stops
        // caring about the result, and joining here would block the redraw.
        thread::spawn(move || {
            let outcome = run_job(request, &cancel, &tx);
            let _ = tx.send(WorkerEvent::Finished(outcome));
        });
        self.events = Some(rx);
        self.status = "Running...".into();
        self.follow_log = true;
        // The egui application switches to Activity when a job starts, so the
        // progress is in front of the operator rather than behind a tab.
        self.select_view(View::Activity);
    }

    /// Act on the focused row.
    pub fn activate(&mut self) {
        let Some(slot) = self.selected() else {
            return;
        };
        match slot.kind() {
            Kind::Toggle => {
                if let Some(flag) = self.toggle_mut(slot) {
                    *flag = !*flag;
                }
            }
            Kind::Text | Kind::Secret => self.move_focus(1),
            Kind::Button => self.press(slot),
        }
    }

    fn press(&mut self, slot: Slot) {
        match slot {
            Slot::TestButton => self.start(Job::Test),
            Slot::AlpineButton => self.start(Job::Alpine),
            Slot::DeployButton => self.start(Job::Deploy),
            Slot::StatusButton => self.start(Job::Manage(ManagementAction::Status)),
            Slot::LogsButton => self.start(Job::Manage(ManagementAction::Logs)),
            Slot::RestartButton => {
                self.pending = Some(Pending {
                    action: ManagementAction::Restart,
                    prompt: "Restart the appliance container now?".into(),
                });
            }
            Slot::RebootButton => {
                self.pending = Some(Pending {
                    action: ManagementAction::Reboot,
                    prompt: "Reboot the Raspberry Pi now?".into(),
                });
            }
            Slot::HostnameButton => self.start(Job::Hostname),
            Slot::WebPasswordButton => self.start(Job::WebPassword),
            Slot::WifiButton => self.start(Job::Wifi),
            _ => {}
        }
    }

    pub fn confirm_pending(&mut self, accepted: bool) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if accepted {
            self.start(Job::Manage(pending.action));
        } else {
            self.status = "Cancelled.".into();
        }
    }

    // Text editing. The cursor counts characters rather than bytes so a
    // multi-byte SSID or password does not split on an edit.

    fn byte_offset(text: &str, cursor: usize) -> usize {
        text.char_indices()
            .nth(cursor)
            .map_or(text.len(), |(index, _)| index)
    }

    pub fn insert(&mut self, character: char) {
        let Some(slot) = self.selected() else { return };
        if !matches!(slot.kind(), Kind::Text | Kind::Secret) {
            return;
        }
        let cursor = self.cursor;
        if let Some(text) = self.value_mut(slot) {
            let at = Self::byte_offset(text, cursor);
            text.insert(at, character);
            self.cursor = cursor + 1;
        }
    }

    pub fn backspace(&mut self) {
        let Some(slot) = self.selected() else { return };
        let cursor = self.cursor;
        if cursor == 0 {
            return;
        }
        if let Some(text) = self.value_mut(slot) {
            let at = Self::byte_offset(text, cursor - 1);
            if at < text.len() {
                text.remove(at);
                self.cursor = cursor - 1;
            }
        }
    }

    pub fn delete(&mut self) {
        let Some(slot) = self.selected() else { return };
        let cursor = self.cursor;
        if let Some(text) = self.value_mut(slot) {
            let at = Self::byte_offset(text, cursor);
            if at < text.len() {
                text.remove(at);
            }
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let Some(slot) = self.selected() else { return };
        let length = self.value(slot).chars().count();
        let current = isize::try_from(self.cursor).unwrap_or(0);
        let next = (current + delta).clamp(0, isize::try_from(length).unwrap_or(0));
        self.cursor = usize::try_from(next).unwrap_or(0);
    }

    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self
            .selected()
            .map_or(0, |slot| self.value(slot).chars().count());
    }
}
