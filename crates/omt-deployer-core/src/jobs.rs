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
    AlpineSetupSettings, Connection, DeployOptions, ManagementAction, Secret, WifiSettings,
    alpine_setup, apply_wifi, change_web_password, deploy, manage, set_hostname, test_connection,
    validate_web_password, validate_wifi,
};
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

/// Every job a deployer frontend runs, all of which talk to the Raspberry Pi.
///
/// There is no local job here: the workstation probe, the package installs,
/// and the ARM64 emulation setup all existed to prepare a machine to *build*
/// the appliance image, and neither frontend builds anything. They carry the
/// image instead. `rpi-omt-deploy prerequisites` and `setup-emulation` still
/// serve a developer rebuilding from a tree.
pub enum Job {
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
