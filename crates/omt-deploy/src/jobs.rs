//! The unit of work a frontend runs on a worker thread. The sequencing here
//! is not always one call into the operations -- Deploy optionally chains a
//! Web password rotation, and several jobs validate before they connect --
//! and holding it in one place keeps a deployment meaning the same thing
//! whichever frontend started it.

use omt_common::{Error, Result};

use crate::capsule::Capsule;
use crate::core::{
    self, Action, AlpineSettings, Connection, DeployOptions, Progress, SdSettings, Secret,
    WifiSettings,
};
use crate::ops;
use crate::sd_card;
use crate::sys::Cancel;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobKind {
    PrepareSd,
    #[default]
    Test,
    Alpine,
    Deploy,
    Manage,
    WebPassword,
    Hostname,
    Wifi,
}

/// Everything a job needs, captured when it starts so later edits to the
/// form cannot change what it does.
#[derive(Clone, Debug)]
pub struct JobRequest {
    pub job: JobKind,
    /// For `Manage`.
    pub action: Action,
    pub connection: Option<Connection>,
    pub options: DeployOptions,
    pub boot_directory: String,
    pub wifi_country: String,
    pub wifi_ssid: String,
    pub wifi_password: Secret,
    pub wifi_connect: bool,
    pub wifi_preserve_existing_profiles: bool,
    pub hostname: String,
    pub manage_hostname: String,
    pub os_root_password: Secret,
    pub os_pi_password: Secret,
    pub rotate_web_password: bool,
    pub web_password: Secret,
}

impl Default for JobRequest {
    fn default() -> Self {
        Self {
            job: JobKind::Test,
            action: Action::Status,
            connection: None,
            options: DeployOptions::default(),
            boot_directory: String::new(),
            wifi_country: "US".into(),
            wifi_ssid: String::new(),
            wifi_password: Secret::absent(),
            wifi_connect: true,
            wifi_preserve_existing_profiles: true,
            hostname: String::new(),
            manage_hostname: String::new(),
            os_root_password: Secret::absent(),
            os_pi_password: Secret::absent(),
            rotate_web_password: false,
            web_password: Secret::absent(),
        }
    }
}

/// The connection fields the terminal application collects, as typed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConnectionFields<'a> {
    pub host: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    pub sudo_password: &'a str,
    pub known_hosts: &'a str,
    pub bootstrap_root_password: &'a str,
}

/// Builds and validates a connection; the validation message on failure.
pub fn connection_from_fields(
    f: &ConnectionFields<'_>,
) -> std::result::Result<Connection, &'static str> {
    let mut c = Connection::new();
    f.host.trim().clone_into(&mut c.host);
    f.username.trim().clone_into(&mut c.username);
    let known = f.known_hosts.trim();
    if !known.is_empty() {
        c.known_hosts_path = Some(known.to_owned());
    }
    // Sent even when empty: untouched factory Alpine accepts root with an
    // empty SSH password, so "" is an explicit credential here.
    c.password = Secret::new(f.password)?;
    if !f.sudo_password.is_empty() {
        c.sudo_password = Secret::new(f.sudo_password)?;
    }
    if !f.bootstrap_root_password.is_empty() {
        c.bootstrap_root_password = Secret::new(f.bootstrap_root_password)?;
    }
    match core::validate_connection(&c) {
        Some(problem) => Err(problem),
        None => Ok(c),
    }
}

fn rotate_web_password(
    r: &JobRequest,
    c: &Connection,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) = core::validate_web_password(&r.web_password) {
        return Err(Error::new(problem));
    }
    ops::change_web_password(c, &r.web_password, cancel, p)
}

pub fn run_job(
    r: &JobRequest,
    capsule: &Capsule,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    let remote = || {
        r.connection
            .as_ref()
            .ok_or_else(|| Error::new("no connection was prepared for this operation"))
    };
    let root = r.options.project_root.as_deref();
    match r.job {
        JobKind::PrepareSd => {
            let s = SdSettings {
                boot_directory: r.boot_directory.clone(),
                country: r.wifi_country.clone(),
                wifi_ssid: r.wifi_ssid.clone(),
                wifi_password: r.wifi_password.clone(),
            };
            sd_card::prepare_sd_card(&s, cancel, p)
        }
        JobKind::Test => ops::test_connection(remote()?, cancel, p),
        JobKind::Alpine => {
            let c = remote()?;
            let wifi = (!r.wifi_ssid.is_empty()).then(|| WifiSettings {
                ssid: r.wifi_ssid.clone(),
                password: r.wifi_password.clone(),
                connect: false,
                preserve_existing_profiles: true,
            });
            let a = AlpineSettings {
                hostname: r.hostname.clone(),
                wifi,
                root_password: r.os_root_password.clone(),
                pi_password: r.os_pi_password.clone(),
            };
            ops::alpine_setup(c, &a, capsule, root, cancel, p)
        }
        JobKind::Deploy => {
            let c = remote()?;
            ops::deploy(c, &r.options, capsule, cancel, p)?;
            if r.rotate_web_password { rotate_web_password(r, c, cancel, p) } else { Ok(()) }
        }
        JobKind::Manage => ops::manage(remote()?, r.action, cancel, p),
        JobKind::WebPassword => rotate_web_password(r, remote()?, cancel, p),
        JobKind::Hostname => {
            ops::set_hostname(remote()?, &r.manage_hostname, capsule, root, cancel, p)
        }
        JobKind::Wifi => {
            let w = WifiSettings {
                ssid: r.wifi_ssid.clone(),
                password: r.wifi_password.clone(),
                connect: r.wifi_connect,
                preserve_existing_profiles: r.wifi_preserve_existing_profiles,
            };
            if let Some(problem) = core::validate_wifi(&w) {
                return Err(Error::new(problem));
            }
            ops::apply_wifi(remote()?, &w, cancel, p)
        }
    }
}
