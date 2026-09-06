// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//! The unit of work a deployer frontend runs on a worker thread.
//!
//! Two frontends drive these operations -- the egui application on Windows and
//! the terminal application on Linux -- and the sequencing here is not always
//! one call into `ops`. Deploy optionally chains a Web password rotation;
//! several jobs validate before they connect. Holding that in one place is
//! what keeps a deployment from meaning something subtly different depending
//! on which program the operator launched.
//!
//! Nothing here draws anything. A frontend supplies a `JobRequest` and a
//! channel, and renders the `WorkerEvent`s that come back.

use crate::{
    AlpineSetupSettings, AuthMethod, Connection, DeployOptions, ManagementAction, SdCardSettings,
    Secret, ValidationError, WifiSettings, alpine_setup, apply_wifi, change_web_password, deploy,
    manage, prepare_sd_card, set_hostname, test_connection, validate_connection,
    validate_web_password, validate_wifi,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use zeroize::Zeroizing;

/// A line of progress, or the outcome.
///
/// The frontend owns the receiving end and decides how to show these; the
/// worker never blocks on whether anyone is listening.
pub enum WorkerEvent {
    Line(String),
    Finished(Result<(), String>),
}

/// Every operation exposed by the native frontends.
///
/// `PrepareSd` is local to the workstation; the remaining jobs use the
/// request's verified SSH connection.
#[derive(Clone, Copy)]
pub enum Job {
    PrepareSd,
    Test,
    Alpine,
    Deploy,
    Manage(ManagementAction),
    WebPassword,
    Hostname,
    Wifi,
}

/// Everything a job needs, captured from the frontend's fields at the moment
/// the operator started it.
///
/// Taken by value so the worker thread cannot observe later edits: a field
/// typed into while a deployment runs must not change what that deployment is
/// doing.
pub struct JobRequest {
    pub job: Job,
    /// Absent only for jobs that never leave this machine.
    pub connection: Option<Connection>,
    /// Carried by every job, not only Deploy: a probe describes a particular
    /// project root and archive, and reading them from anywhere else would let
    /// the frontend answer for one while the deployment used another.
    pub options: DeployOptions,
    pub boot_directory: PathBuf,
    pub wifi_country: String,
    pub wifi_ssid: String,
    pub wifi_password: Zeroizing<String>,
    pub wifi_connect: bool,
    pub wifi_preserve_existing_profiles: bool,
    pub hostname: String,
    pub manage_hostname: String,
    pub os_root_password: Zeroizing<String>,
    pub os_pi_password: Zeroizing<String>,
    pub rotate_web_password: bool,
    pub web_password: Zeroizing<String>,
}

impl Default for JobRequest {
    fn default() -> Self {
        Self {
            job: Job::Test,
            connection: None,
            options: DeployOptions::default(),
            boot_directory: PathBuf::new(),
            wifi_country: "US".into(),
            wifi_ssid: String::new(),
            wifi_password: Zeroizing::new(String::new()),
            wifi_connect: true,
            wifi_preserve_existing_profiles: true,
            hostname: String::new(),
            manage_hostname: String::new(),
            os_root_password: Zeroizing::new(String::new()),
            os_pi_password: Zeroizing::new(String::new()),
            rotate_web_password: false,
            web_password: Zeroizing::new(String::new()),
        }
    }
}

/// The connection fields both native frontends collect, as typed.
///
/// The terminal and egui applications each grew their own builder, and the two
/// had come to disagree: one trimmed the host and the user, the other did not,
/// so a pasted host with a trailing space left the egui application refusing
/// to connect with nothing on screen to say why. Neither exposes a port or key
/// authentication; the CLI, which does, builds its own [`Connection`].
pub struct ConnectionFields<'a> {
    pub host: &'a str,
    pub username: &'a str,
    /// Sent even when empty. Untouched factory Alpine accepts root with an
    /// empty SSH password, and the SSH adapter deliberately tries `none`,
    /// password, and keyboard-interactive for that case: an empty string here
    /// is an explicit credential, not a missing one.
    pub password: &'a str,
    /// Empty means no separate sudo credential rather than an empty one.
    pub sudo_password: &'a str,
    /// Empty uses the operator's own `~/.ssh/known_hosts`.
    pub known_hosts: &'a str,
    /// The Alpine view's root password, used once through `su` to install bash
    /// and sudo when the SSH account is not root.
    pub bootstrap_root_password: &'a str,
}

/// The verified [`Connection`] a frontend's fields describe.
pub fn connection_from_fields(
    fields: &ConnectionFields<'_>,
) -> Result<Connection, ValidationError> {
    let optional = |value: &str| -> Result<Option<Secret>, ValidationError> {
        if value.is_empty() {
            Ok(None)
        } else {
            Secret::new(value.to_owned()).map(Some)
        }
    };
    let known_hosts = fields.known_hosts.trim();
    let connection = Connection {
        host: fields.host.trim().to_owned(),
        username: fields.username.trim().to_owned(),
        port: 22,
        auth: AuthMethod::Password,
        password: Some(Secret::new(fields.password.to_owned())?),
        key_path: None,
        key_passphrase: None,
        known_hosts_path: if known_hosts.is_empty() {
            None
        } else {
            Some(PathBuf::from(known_hosts))
        },
        sudo_password: optional(fields.sudo_password)?,
        bootstrap_root_password: optional(fields.bootstrap_root_password)?,
    };
    validate_connection(&connection)?;
    Ok(connection)
}

/// Run one job to completion, reporting progress as it goes.
///
/// Returns the outcome rather than sending it: the caller owns the thread and
/// decides how a finish is announced, which differs between a repainting GUI
/// and a terminal that redraws on its own schedule.
pub fn run_job(
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
    // Every remote job proved its connection before the worker started; this
    // is the one place that unwrapping is expressed as an error.
    let remote = || {
        request
            .connection
            .as_ref()
            .ok_or_else(|| "no connection was prepared for this operation".to_owned())
    };
    match request.job {
        Job::PrepareSd => {
            let settings = SdCardSettings {
                boot_directory: request.boot_directory,
                country: request.wifi_country,
                wifi_ssid: request.wifi_ssid,
                wifi_password: Secret::new((*request.wifi_password).clone())
                    .map_err(|error| error.to_string())?,
            };
            prepare_sd_card(&settings, cancel, &mut progress).map_err(|error| error.to_string())
        }
        Job::Test => {
            test_connection(remote()?, cancel, &mut progress).map_err(|error| error.to_string())
        }
        Job::Alpine => {
            let wifi = if request.wifi_ssid.is_empty() {
                None
            } else {
                Some(WifiSettings {
                    ssid: request.wifi_ssid.clone(),
                    password: Secret::new((*request.wifi_password).clone())
                        .map_err(|error| error.to_string())?,
                    connect: false,
                    preserve_existing_profiles: true,
                })
            };
            let settings = AlpineSetupSettings {
                hostname: request.hostname,
                wifi,
                root_password: Secret::new((*request.os_root_password).clone())
                    .map_err(|error| error.to_string())?,
                pi_password: Secret::new((*request.os_pi_password).clone())
                    .map_err(|error| error.to_string())?,
            };
            alpine_setup(
                remote()?,
                &settings,
                request.options.project_root.as_deref(),
                cancel,
                &mut progress,
            )
            .map_err(|error| error.to_string())
        }
        Job::Deploy => {
            deploy(remote()?, &request.options, cancel, &mut progress)
                .map_err(|error| error.to_string())?;
            if !request.rotate_web_password {
                return Ok(());
            }
            let password =
                Secret::new((*request.web_password).clone()).map_err(|error| error.to_string())?;
            validate_web_password(&password).map_err(|error| error.to_string())?;
            change_web_password(remote()?, &password, cancel, &mut progress)
                .map_err(|error| error.to_string())
        }
        Job::Manage(action) => manage(remote()?, action, cancel, &mut progress)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Job::WebPassword => {
            let password =
                Secret::new((*request.web_password).clone()).map_err(|error| error.to_string())?;
            validate_web_password(&password).map_err(|error| error.to_string())?;
            change_web_password(remote()?, &password, cancel, &mut progress)
                .map_err(|error| error.to_string())
        }
        Job::Hostname => set_hostname(
            remote()?,
            &request.manage_hostname,
            request.options.project_root.as_deref(),
            cancel,
            &mut progress,
        )
        .map_err(|error| error.to_string()),
        Job::Wifi => {
            let settings = WifiSettings {
                ssid: request.wifi_ssid,
                password: Secret::new((*request.wifi_password).clone())
                    .map_err(|error| error.to_string())?,
                connect: request.wifi_connect,
                preserve_existing_profiles: request.wifi_preserve_existing_profiles,
            };
            validate_wifi(&settings).map_err(|error| error.to_string())?;
            apply_wifi(remote()?, &settings, cancel, &mut progress)
                .map_err(|error| error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionFields, connection_from_fields};
    use crate::Secret;

    fn fields<'a>() -> ConnectionFields<'a> {
        ConnectionFields {
            host: "pi.local",
            username: "pi",
            password: "ssh-password",
            sudo_password: "",
            known_hosts: "",
            bootstrap_root_password: "",
        }
    }

    /// Three credentials that must never be mistaken for each other: the SSH
    /// password, the sudo password for a non-root account, and the Alpine root
    /// password first Deploy uses through `su`.
    #[test]
    fn the_three_credentials_stay_separate() {
        let connection = connection_from_fields(&ConnectionFields {
            sudo_password: "sudo-password",
            bootstrap_root_password: "root-password",
            ..fields()
        })
        .unwrap_or_else(|error| panic!("{error}"));
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

    /// An empty optional field is an absent credential; an empty SSH password
    /// is a real one, because that is how a factory Alpine image answers.
    #[test]
    fn an_empty_ssh_password_survives_where_the_optional_ones_do_not() {
        let connection = connection_from_fields(&ConnectionFields {
            username: "root",
            password: "",
            ..fields()
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(connection.password.as_ref().map(Secret::expose), Some(""));
        assert!(connection.sudo_password.is_none());
        assert!(connection.bootstrap_root_password.is_none());
        assert!(connection.known_hosts_path.is_none());
    }

    /// A host pasted from a terminal or a label carries whitespace, and
    /// `valid_host` rejects it. Trimming here is what keeps the two frontends
    /// from disagreeing about whether such a host is usable.
    #[test]
    fn surrounding_whitespace_is_trimmed_from_the_typed_fields() {
        let connection = connection_from_fields(&ConnectionFields {
            host: "  pi.local\t",
            username: " pi ",
            ..fields()
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(connection.host, "pi.local");
        assert_eq!(connection.username, "pi");
    }

    /// Trimming is not repair: a host that is invalid once trimmed is still
    /// refused here rather than at the far end of a connection attempt.
    #[test]
    fn an_invalid_host_is_refused_before_anything_connects() {
        assert!(
            connection_from_fields(&ConnectionFields {
                host: " -pi.local ",
                ..fields()
            })
            .is_err()
        );
        assert!(
            connection_from_fields(&ConnectionFields {
                username: "ro ot",
                ..fields()
            })
            .is_err()
        );
        assert!(
            connection_from_fields(&ConnectionFields {
                known_hosts: "/nonexistent/known_hosts",
                ..fields()
            })
            .is_err()
        );
    }
}
