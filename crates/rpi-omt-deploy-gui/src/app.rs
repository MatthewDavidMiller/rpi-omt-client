//! The deployer's state: the form, the prechecks, the confirmations, and
//! the one job that may run. The browser page is a view of this; nothing
//! here knows about HTTP. The field set is the one docs/SETUP.md describes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use omt_common::buf::wipe_string;
use omt_deploy::capsule::Capsule;
use omt_deploy::core::{self, Action, Connection, Secret};
use omt_deploy::jobs::{self, ConnectionFields, JobKind, JobRequest};

/// How many progress lines the activity log keeps: a deployment emits far
/// more than a screen's worth, and the operator scrolling back wants the
/// recent end.
pub const LOG_CAPACITY: usize = 2000;
pub const FIELD_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Connection,
    SdCard,
    Alpine,
    Deploy,
    Manage,
    Wifi,
    Activity,
    About,
}

pub const VIEWS: [View; 8] = [
    View::Connection,
    View::SdCard,
    View::Alpine,
    View::Deploy,
    View::Manage,
    View::Wifi,
    View::Activity,
    View::About,
];

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Connection => "Connection",
            View::SdCard => "SD card",
            View::Alpine => "Alpine",
            View::Deploy => "Deploy",
            View::Manage => "Manage",
            View::Wifi => "Wi-Fi",
            View::Activity => "Activity",
            View::About => "About",
        }
    }

    /// The rows a view shows, in order.
    // Every arm lists slots; the glob keeps the tables readable.
    #[allow(clippy::enum_glob_use)]
    pub fn slots(self) -> &'static [Slot] {
        use Slot::*;
        match self {
            View::Connection => &[Host, User, Password, SudoPassword, KnownHosts, TestButton],
            View::SdCard => {
                &[BootDirectory, SdWifiCountry, SdWifiSsid, SdWifiPassword, PrepareSdButton]
            }
            View::Alpine => &[
                AlpineHostname,
                AlpineRootPassword,
                AlpineRootConfirm,
                AlpinePiPassword,
                AlpinePiConfirm,
                AlpineWifiSsid,
                AlpineWifiPassword,
                AlpineApplyLogin,
                AlpineButton,
            ],
            View::Deploy => {
                &[RemoteDirectory, RotateWebPassword, WebPassword, WebConfirm, DeployButton]
            }
            View::Manage => &[
                StatusButton,
                LogsButton,
                RestartButton,
                RebootButton,
                ManageHostname,
                HostnameButton,
                ManageWebPassword,
                ManageWebConfirm,
                WebPasswordButton,
            ],
            View::Wifi => &[WifiSsid, WifiPassword, WifiConnect, WifiPreserve, WifiButton],
            View::Activity | View::About => &[],
        }
    }
}

/// One row of a view, flat across every view, so the page and the server
/// name the same thing the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Host,
    User,
    Password,
    SudoPassword,
    KnownHosts,
    TestButton,
    BootDirectory,
    SdWifiCountry,
    SdWifiSsid,
    SdWifiPassword,
    PrepareSdButton,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Secret,
    Toggle,
    Button,
}

/// The editable values. Several slots share one: the Wi-Fi SSID and
/// password are the same fields on the SD-card, Alpine, and Wi-Fi views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Host,
    User,
    Password,
    SudoPassword,
    KnownHosts,
    BootDirectory,
    WifiCountry,
    Hostname,
    OsRootPassword,
    OsRootConfirm,
    OsPiPassword,
    OsPiConfirm,
    RemoteDirectory,
    WebPassword,
    WebConfirm,
    ManageHostname,
    ManageWebPassword,
    ManageWebConfirm,
    WifiSsid,
    WifiPassword,
}

const FIELD_COUNT: usize = 20;

pub const FIELDS: [Field; FIELD_COUNT] = [
    Field::Host,
    Field::User,
    Field::Password,
    Field::SudoPassword,
    Field::KnownHosts,
    Field::BootDirectory,
    Field::WifiCountry,
    Field::Hostname,
    Field::OsRootPassword,
    Field::OsRootConfirm,
    Field::OsPiPassword,
    Field::OsPiConfirm,
    Field::RemoteDirectory,
    Field::WebPassword,
    Field::WebConfirm,
    Field::ManageHostname,
    Field::ManageWebPassword,
    Field::ManageWebConfirm,
    Field::WifiSsid,
    Field::WifiPassword,
];

impl Field {
    /// The name the page addresses this value by.
    pub fn name(self) -> &'static str {
        match self {
            Field::Host => "host",
            Field::User => "user",
            Field::Password => "password",
            Field::SudoPassword => "sudo_password",
            Field::KnownHosts => "known_hosts",
            Field::BootDirectory => "boot_directory",
            Field::WifiCountry => "wifi_country",
            Field::Hostname => "hostname",
            Field::OsRootPassword => "os_root_password",
            Field::OsRootConfirm => "os_root_confirm",
            Field::OsPiPassword => "os_pi_password",
            Field::OsPiConfirm => "os_pi_confirm",
            Field::RemoteDirectory => "remote_directory",
            Field::WebPassword => "web_password",
            Field::WebConfirm => "web_confirm",
            Field::ManageHostname => "manage_hostname",
            Field::ManageWebPassword => "manage_web_password",
            Field::ManageWebConfirm => "manage_web_confirm",
            Field::WifiSsid => "wifi_ssid",
            Field::WifiPassword => "wifi_password",
        }
    }

    pub fn from_name(name: &str) -> Option<Field> {
        FIELDS.into_iter().find(|f| f.name() == name)
    }

    pub fn is_secret(self) -> bool {
        is_secret(self)
    }
}

impl Slot {
    pub fn info(self) -> (&'static str, Kind, Option<Field>) {
        use Slot as S;
        match self {
            S::Host => ("Host", Kind::Text, Some(Field::Host)),
            S::User => ("Username", Kind::Text, Some(Field::User)),
            S::Password => ("SSH password", Kind::Secret, Some(Field::Password)),
            S::SudoPassword => {
                ("sudo password (optional)", Kind::Secret, Some(Field::SudoPassword))
            }
            S::KnownHosts => ("known_hosts path (optional)", Kind::Text, Some(Field::KnownHosts)),
            S::TestButton => ("Test connection", Kind::Button, None),
            S::BootDirectory => {
                ("Alpine boot partition path", Kind::Text, Some(Field::BootDirectory))
            }
            S::SdWifiCountry => ("Wi-Fi country", Kind::Text, Some(Field::WifiCountry)),
            S::SdWifiSsid => ("Initial Wi-Fi SSID", Kind::Text, Some(Field::WifiSsid)),
            S::SdWifiPassword => {
                ("Initial Wi-Fi password", Kind::Secret, Some(Field::WifiPassword))
            }
            S::PrepareSdButton => ("Prepare SD card", Kind::Button, None),
            S::AlpineHostname => ("Appliance hostname", Kind::Text, Some(Field::Hostname)),
            S::AlpineRootPassword => {
                ("New root password", Kind::Secret, Some(Field::OsRootPassword))
            }
            S::AlpineRootConfirm => {
                ("Confirm root password", Kind::Secret, Some(Field::OsRootConfirm))
            }
            S::AlpinePiPassword => ("New pi password", Kind::Secret, Some(Field::OsPiPassword)),
            S::AlpinePiConfirm => ("Confirm pi password", Kind::Secret, Some(Field::OsPiConfirm)),
            S::AlpineWifiSsid => ("Wi-Fi SSID (optional)", Kind::Text, Some(Field::WifiSsid)),
            S::AlpineApplyLogin => ("Use the new pi login after setup", Kind::Toggle, None),
            S::AlpineButton => ("Run Alpine setup", Kind::Button, None),
            S::RemoteDirectory => ("Remote directory", Kind::Text, Some(Field::RemoteDirectory)),
            S::RotateWebPassword => ("Also set the Web GUI password", Kind::Toggle, None),
            S::WebPassword => ("Web GUI password", Kind::Secret, Some(Field::WebPassword)),
            S::WebConfirm => ("Confirm Web GUI password", Kind::Secret, Some(Field::WebConfirm)),
            S::DeployButton => ("Deploy", Kind::Button, None),
            S::StatusButton => ("Status", Kind::Button, None),
            S::LogsButton => ("Logs", Kind::Button, None),
            S::RestartButton => ("Restart the appliance", Kind::Button, None),
            S::RebootButton => ("Reboot the Raspberry Pi", Kind::Button, None),
            S::ManageHostname => ("Rename appliance to", Kind::Text, Some(Field::ManageHostname)),
            S::HostnameButton => ("Apply hostname", Kind::Button, None),
            S::ManageWebPassword => {
                ("New Web GUI password", Kind::Secret, Some(Field::ManageWebPassword))
            }
            S::ManageWebConfirm => {
                ("Confirm Web GUI password", Kind::Secret, Some(Field::ManageWebConfirm))
            }
            S::WebPasswordButton => ("Change Web GUI password", Kind::Button, None),
            S::WifiSsid => ("SSID", Kind::Text, Some(Field::WifiSsid)),
            S::AlpineWifiPassword | S::WifiPassword => {
                ("Wi-Fi password", Kind::Secret, Some(Field::WifiPassword))
            }
            S::WifiConnect => ("Connect now", Kind::Toggle, None),
            S::WifiPreserve => ("Keep other saved profiles", Kind::Toggle, None),
            S::WifiButton => ("Apply Wi-Fi settings", Kind::Button, None),
        }
    }

    pub fn label(self) -> &'static str {
        self.info().0
    }

    pub fn kind(self) -> Kind {
        self.info().1
    }

    pub fn field(self) -> Option<Field> {
        self.info().2
    }

    /// The name the page addresses this row by.
    // One arm per slot; the glob keeps the table readable.
    #[allow(clippy::enum_glob_use)]
    pub fn name(self) -> &'static str {
        use Slot::*;
        match self {
            Host => "host",
            User => "user",
            Password => "password",
            SudoPassword => "sudo_password",
            KnownHosts => "known_hosts",
            TestButton => "test",
            BootDirectory => "boot_directory",
            SdWifiCountry => "sd_wifi_country",
            SdWifiSsid => "sd_wifi_ssid",
            SdWifiPassword => "sd_wifi_password",
            PrepareSdButton => "prepare_sd",
            AlpineHostname => "alpine_hostname",
            AlpineRootPassword => "alpine_root_password",
            AlpineRootConfirm => "alpine_root_confirm",
            AlpinePiPassword => "alpine_pi_password",
            AlpinePiConfirm => "alpine_pi_confirm",
            AlpineWifiSsid => "alpine_wifi_ssid",
            AlpineWifiPassword => "alpine_wifi_password",
            AlpineApplyLogin => "alpine_apply_login",
            AlpineButton => "alpine",
            RemoteDirectory => "remote_directory",
            RotateWebPassword => "rotate_web_password",
            WebPassword => "web_password",
            WebConfirm => "web_confirm",
            DeployButton => "deploy",
            StatusButton => "status",
            LogsButton => "logs",
            RestartButton => "restart",
            RebootButton => "reboot",
            ManageHostname => "manage_hostname",
            HostnameButton => "apply_hostname",
            ManageWebPassword => "manage_web_password",
            ManageWebConfirm => "manage_web_confirm",
            WebPasswordButton => "change_web_password",
            WifiSsid => "wifi_ssid",
            WifiPassword => "wifi_password",
            WifiConnect => "wifi_connect",
            WifiPreserve => "wifi_preserve",
            WifiButton => "apply_wifi",
        }
    }

    pub fn from_name(name: &str) -> Option<Slot> {
        VIEWS.iter().flat_map(|v| v.slots()).copied().find(|s| s.name() == name)
    }

    /// What pressing a button runs, and the question asked first when it
    /// cannot be undone.
    pub fn job(self) -> Option<(JobKind, Action, Option<&'static str>)> {
        Some(match self {
            Slot::TestButton => (JobKind::Test, Action::Status, None),
            Slot::PrepareSdButton => (JobKind::PrepareSd, Action::Status, None),
            // This erases the disk the Pi boots from.
            Slot::AlpineButton => (
                JobKind::Alpine,
                Action::Status,
                Some(
                    "Erase the boot disk and install Alpine in persistent sys mode? The Pi \
                     reboots when it finishes.",
                ),
            ),
            Slot::DeployButton => (JobKind::Deploy, Action::Status, None),
            Slot::StatusButton => (JobKind::Manage, Action::Status, None),
            Slot::LogsButton => (JobKind::Manage, Action::Logs, None),
            Slot::RestartButton => {
                (JobKind::Manage, Action::Restart, Some("Restart the appliance container now?"))
            }
            Slot::RebootButton => {
                (JobKind::Manage, Action::Reboot, Some("Reboot the Raspberry Pi now?"))
            }
            Slot::HostnameButton => (JobKind::Hostname, Action::Status, None),
            Slot::WebPasswordButton => (JobKind::WebPassword, Action::Status, None),
            Slot::WifiButton => (JobKind::Wifi, Action::Status, None),
            _ => return None,
        })
    }
}

/// What answering a confirmation yes will do.
#[derive(Clone, Debug)]
pub struct Pending {
    /// Otherwise start `job`.
    pub quit: bool,
    pub job: JobKind,
    pub action: Action,
    pub prompt: String,
}

/// Shared between the frontend and one job's thread.
#[derive(Debug, Default)]
struct WorkerState {
    lines: Vec<String>,
    /// The job's outcome, once it has finished.
    result: Option<Result<(), String>>,
}

#[derive(Debug)]
pub struct Worker {
    state: Arc<Mutex<WorkerState>>,
    cancel: Arc<AtomicBool>,
}

impl Worker {
    /// For the tests: a worker that has already finished.
    pub fn finished(result: Result<(), &str>) -> Self {
        let state = WorkerState { lines: Vec::new(), result: Some(result.map_err(str::to_owned)) };
        Self { state: Arc::new(Mutex::new(state)), cancel: Arc::new(AtomicBool::new(false)) }
    }

    /// For the tests: a worker still running.
    pub fn running() -> Self {
        Self { state: Arc::default(), cancel: Arc::new(AtomicBool::new(false)) }
    }

    pub fn cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct App {
    pub capsule: &'static Capsule,
    pub should_quit: bool,
    pub status: String,

    fields: [String; FIELD_COUNT],
    pub apply_alpine_login: bool,
    pub rotate_web_password: bool,
    pub wifi_connect: bool,
    pub wifi_preserve: bool,

    pub log: std::collections::VecDeque<String>,
    /// How many lines the log has ever held: the sequence number of the
    /// line after the newest, so a reader can ask for only what it has not
    /// seen even after the oldest lines have been dropped.
    pub log_end: u64,
    pub confirm: Option<Pending>,

    pub job: Option<Worker>,
    pub switch_to_pi_after_job: bool,
}

fn is_secret(f: Field) -> bool {
    matches!(
        f,
        Field::Password
            | Field::SudoPassword
            | Field::OsRootPassword
            | Field::OsRootConfirm
            | Field::OsPiPassword
            | Field::OsPiConfirm
            | Field::WebPassword
            | Field::WebConfirm
            | Field::ManageWebPassword
            | Field::ManageWebConfirm
            | Field::WifiPassword
    )
}

impl Drop for App {
    fn drop(&mut self) {
        for f in &mut self.fields {
            wipe_string(f);
        }
    }
}

impl App {
    pub fn new(capsule: &'static Capsule) -> Self {
        let mut a = Self {
            capsule,
            should_quit: false,
            status: "Ready.".into(),
            fields: Default::default(),
            apply_alpine_login: false,
            rotate_web_password: false,
            wifi_connect: true,
            wifi_preserve: true,
            log: std::collections::VecDeque::new(),
            log_end: 0,
            confirm: None,
            job: None,
            switch_to_pi_after_job: false,
        };
        a.set_field(Field::Host, "raspberrypi.local");
        a.set_field(Field::User, "root");
        a.set_field(Field::WifiCountry, "US");
        a.set_field(Field::Hostname, "omt-client");
        a.set_field(Field::RemoteDirectory, "/opt/omt-client");
        a
    }

    pub fn field(&self, f: Field) -> &str {
        &self.fields[f as usize]
    }

    /// Replaces a value, wiping a secret's old characters first. Control
    /// characters never reach a value, and nothing longer than
    /// `FIELD_LIMIT` bytes is kept.
    pub fn set_field(&mut self, f: Field, text: &str) -> bool {
        if text.len() > FIELD_LIMIT || text.chars().any(char::is_control) {
            return false;
        }
        let slot = &mut self.fields[f as usize];
        if is_secret(f) {
            wipe_string(slot);
        }
        text.clone_into(slot);
        true
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn toggle(&self, s: Slot) -> bool {
        match s {
            Slot::AlpineApplyLogin => self.apply_alpine_login,
            Slot::RotateWebPassword => self.rotate_web_password,
            Slot::WifiConnect => self.wifi_connect,
            Slot::WifiPreserve => self.wifi_preserve,
            _ => false,
        }
    }

    /// Sets a toggle; false when `s` is not one.
    pub fn set_toggle(&mut self, s: Slot, on: bool) -> bool {
        let flag = match s {
            Slot::AlpineApplyLogin => &mut self.apply_alpine_login,
            Slot::RotateWebPassword => &mut self.rotate_web_password,
            Slot::WifiConnect => &mut self.wifi_connect,
            Slot::WifiPreserve => &mut self.wifi_preserve,
            _ => return false,
        };
        *flag = on;
        true
    }

    /// The status line is one row. Remote errors arrive with line breaks in
    /// them, and a break would end the row partway through the message.
    pub fn set_status(&mut self, text: &str) {
        let mut out = String::with_capacity(text.len());
        let mut in_break = false;
        for c in text.chars() {
            if c == '\n' || c == '\r' {
                in_break = true;
                continue;
            }
            if in_break && !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            in_break = false;
            out.push(c);
        }
        out.truncate(out.trim_end_matches(' ').len());
        self.status = out;
    }

    fn push_log_line(&mut self, line: &str) {
        if self.log.len() >= LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(line.to_owned());
        self.log_end += 1;
    }

    /// One log row per line: a remote error carries its own line breaks,
    /// and a row holding one would render it as a replacement character.
    pub fn push_log(&mut self, text: &str) {
        let mut rest = text;
        loop {
            let (line, next) = match rest.find('\n') {
                Some(nl) => (&rest[..nl], Some(&rest[nl + 1..])),
                None => (rest, None),
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            self.push_log_line(line);
            match next {
                // A trailing break ends the last line rather than starting an
                // empty one.
                None | Some("") => return,
                Some(n) => rest = n,
            }
        }
    }

    /// The lines from sequence number `since` on, and the number the first
    /// of them carries: later than `since` when older lines have been
    /// dropped.
    pub fn log_since(&self, since: u64) -> (u64, impl Iterator<Item = &String>) {
        let start = self.log_end - self.log.len() as u64;
        let first = since.clamp(start, self.log_end);
        (first, self.log.iter().skip((first - start) as usize))
    }

    /// Drains whatever the worker has produced since the last look.
    pub fn poll_worker(&mut self) {
        let Some(w) = &self.job else { return };
        let (lines, result) = {
            let mut st = w.state.lock().unwrap_or_else(PoisonError::into_inner);
            (std::mem::take(&mut st.lines), st.result.take())
        };
        for line in &lines {
            self.push_log(line);
        }
        let Some(result) = result else { return };
        self.job = None;
        match result {
            Ok(()) => {
                self.set_status("Finished successfully.");
                self.push_log("-- finished successfully --");
                if self.switch_to_pi_after_job {
                    self.set_field(Field::User, "pi");
                    let pi = self.field(Field::OsPiPassword).to_owned();
                    self.set_field(Field::Password, &pi);
                    self.set_field(Field::SudoPassword, &pi);
                    self.push_log(
                        "Connection updated to user pi. Deploy next; the Alpine root password \
                         installs sudo on first deploy.",
                    );
                }
            }
            Err(error) => {
                let text = error.trim_end_matches(['\n', '\r', ' ']);
                self.set_status(&format!("Failed: {text}"));
                self.push_log(&format!("-- failed: {text} --"));
            }
        }
        self.switch_to_pi_after_job = false;
    }

    pub fn cancel_job(&mut self) {
        let Some(w) = &self.job else { return };
        w.cancel.store(true, Ordering::Relaxed);
        self.set_status("Cancelling...");
        self.push_log("-- cancellation requested --");
    }

    fn trimmed(&self, f: Field) -> String {
        self.field(f).trim().to_owned()
    }

    /// The connection the remote jobs share: an always-present SSH password
    /// (factory Alpine answers root with an empty one), optional sudo and
    /// known_hosts, and the Alpine view's root password as the bootstrap
    /// secret.
    fn build_connection(&self) -> Result<Connection, &'static str> {
        jobs::connection_from_fields(&ConnectionFields {
            host: self.field(Field::Host),
            username: self.field(Field::User),
            password: self.field(Field::Password),
            sudo_password: self.field(Field::SudoPassword),
            known_hosts: self.field(Field::KnownHosts),
            bootstrap_root_password: self.field(Field::OsRootPassword),
        })
    }

    /// Validates what the operator can still fix; `None` when the job may
    /// start.
    pub fn precheck(&self, job: JobKind) -> Option<&'static str> {
        match job {
            JobKind::PrepareSd => {
                let password = match Secret::new(self.field(Field::WifiPassword)) {
                    Ok(p) => p,
                    Err(problem) => return Some(problem),
                };
                let s = core::SdSettings {
                    boot_directory: self.trimmed(Field::BootDirectory),
                    country: self.trimmed(Field::WifiCountry),
                    wifi_ssid: self.trimmed(Field::WifiSsid),
                    wifi_password: password,
                };
                omt_deploy::sd_card::validate_sd_settings(&s)
            }
            JobKind::Alpine => {
                if !core::valid_appliance_hostname(&self.trimmed(Field::Hostname)) {
                    return Some("Appliance hostname must be one DNS label of 1-63 characters");
                }
                if self.field(Field::OsRootPassword) != self.field(Field::OsRootConfirm) {
                    return Some("Root password confirmation does not match");
                }
                if self.field(Field::OsPiPassword) != self.field(Field::OsPiConfirm) {
                    return Some("pi password confirmation does not match");
                }
                for f in [Field::OsRootPassword, Field::OsPiPassword] {
                    let problem = Secret::new(self.field(f))
                        .map_or_else(Some, |s| core::validate_os_password(&s));
                    if problem.is_some() {
                        return problem;
                    }
                }
                if !self.trimmed(Field::WifiSsid).is_empty()
                    && self.field(Field::WifiPassword).is_empty()
                {
                    return Some("A Wi-Fi SSID needs a Wi-Fi password");
                }
                None
            }
            JobKind::Deploy => {
                if !core::valid_remote_directory(&self.trimmed(Field::RemoteDirectory)) {
                    return Some(
                        "Remote directory must be a normalized absolute path, such as /opt/omt-client",
                    );
                }
                if self.rotate_web_password
                    && self.field(Field::WebPassword) != self.field(Field::WebConfirm)
                {
                    return Some("Web GUI password confirmation does not match");
                }
                None
            }
            JobKind::WebPassword => (self.field(Field::ManageWebPassword)
                != self.field(Field::ManageWebConfirm))
            .then_some("Web GUI password confirmation does not match"),
            JobKind::Hostname => {
                (!core::valid_appliance_hostname(&self.trimmed(Field::ManageHostname)))
                    .then_some("Hostname must be one DNS label of 1-63 characters")
            }
            JobKind::Test | JobKind::Manage | JobKind::Wifi => None,
        }
    }

    /// The request a job would run with.
    pub fn request(&self, job: JobKind, action: Action) -> Result<JobRequest, &'static str> {
        let mut r = JobRequest { job, action, ..JobRequest::default() };
        r.options.remote_directory = self.trimmed(Field::RemoteDirectory);
        if job != JobKind::PrepareSd {
            r.connection = Some(self.build_connection()?);
        }
        r.boot_directory = self.trimmed(Field::BootDirectory);
        r.wifi_country = self.trimmed(Field::WifiCountry);
        r.wifi_ssid = self.trimmed(Field::WifiSsid);
        r.wifi_connect = self.wifi_connect;
        r.wifi_preserve_existing_profiles = self.wifi_preserve;
        r.hostname = self.trimmed(Field::Hostname);
        r.manage_hostname = self.trimmed(Field::ManageHostname);
        r.rotate_web_password = self.rotate_web_password;
        // Manage's Web password fields are separate from Deploy's, so a
        // rotation typed on one view cannot be submitted from the other.
        let web =
            if job == JobKind::WebPassword { Field::ManageWebPassword } else { Field::WebPassword };
        r.wifi_password = Secret::new(self.field(Field::WifiPassword))?;
        r.os_root_password = Secret::new(self.field(Field::OsRootPassword))?;
        r.os_pi_password = Secret::new(self.field(Field::OsPiPassword))?;
        r.web_password = Secret::new(self.field(web))?;
        Ok(r)
    }

    /// Starts a job on a worker thread.
    pub fn start(&mut self, job: JobKind, action: Action) {
        if self.busy() {
            self.set_status("A job is already running.");
            return;
        }
        if let Some(problem) = self.precheck(job) {
            self.set_status(&format!("Cannot start: {problem}"));
            return;
        }
        let request = match self.request(job, action) {
            Ok(r) => r,
            Err(problem) => {
                self.set_status(&format!("Cannot start: {problem}"));
                return;
            }
        };
        let worker = Worker::running();
        let state = Arc::clone(&worker.state);
        let cancel = Arc::clone(&worker.cancel);
        let capsule = self.capsule;
        let spawned = thread::Builder::new()
            .name("deploy-job".into())
            // Key derivation and the SSH packet buffers want headroom.
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let progress = |line: &str| {
                    state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .lines
                        .push(line.to_owned());
                };
                let outcome = jobs::run_job(&request, capsule, Some(&cancel), Some(&progress));
                // The request holds secrets; they are wiped as it drops here.
                drop(request);
                state.lock().unwrap_or_else(PoisonError::into_inner).result =
                    Some(outcome.map_err(|e| e.message().to_owned()));
            });
        if let Err(e) = spawned {
            self.set_status(&format!("Cannot start: cannot start a worker thread: {e}"));
            return;
        }
        self.job = Some(worker);
        self.switch_to_pi_after_job = job == JobKind::Alpine && self.apply_alpine_login;
        self.set_status("Running...");
    }

    fn ask(&mut self, quit: bool, job: JobKind, action: Action, prompt: &str) {
        self.confirm =
            Some(Pending { quit, job, action, prompt: prompt.chars().take(255).collect() });
    }

    /// Why pressing `s` would not start its job right now, or `None` when
    /// it would. The page shows this beside the button.
    pub fn blocked(&self, s: Slot) -> Option<&'static str> {
        let (job, action, _) = s.job()?;
        if let Some(problem) = self.precheck(job) {
            return Some(problem);
        }
        // Builds, then drops (and wipes), the request the job would run.
        self.request(job, action).err()
    }

    pub fn press(&mut self, s: Slot) {
        let Some((job, action, prompt)) = s.job() else { return };
        match prompt {
            Some(prompt) => self.ask(false, job, action, prompt),
            None => self.start(job, action),
        }
    }

    /// Quitting abandons a worker that may be mid-transaction, so it asks
    /// first while one runs.
    pub fn request_quit(&mut self) {
        if self.busy() {
            self.ask(
                true,
                JobKind::Test,
                Action::Status,
                "A job is still running. Quit and stop watching it?",
            );
        } else {
            self.should_quit = true;
        }
    }

    pub fn confirm_pending(&mut self, accepted: bool) {
        let Some(p) = self.confirm.take() else { return };
        if !accepted {
            self.set_status("Cancelled.");
            return;
        }
        if p.quit {
            self.should_quit = true;
        } else {
            self.start(p.job, p.action);
        }
    }

    /// For the tests: install a worker as if a job had started.
    pub fn set_worker(&mut self, w: Worker) {
        self.job = Some(w);
    }
}
