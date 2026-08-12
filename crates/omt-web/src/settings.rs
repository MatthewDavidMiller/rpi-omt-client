use std::{env, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimit {
    pub count: usize,
    pub window: Duration,
}

impl RateLimit {
    fn parse(name: &str, default: &str) -> Result<Self, String> {
        let raw = env::var(name).unwrap_or_else(|_| default.to_owned());
        let words: Vec<_> = raw.split_whitespace().collect();
        if words.len() != 3 || words[1] != "per" {
            return Err(format!(
                "{name} must be a rate limit such as {default:?}; received {raw:?}"
            ));
        }
        let count = words[0]
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("{name} has an invalid request count"))?;
        let unit = words[2].trim_end_matches('s');
        let seconds = match unit {
            "second" => 1,
            "minute" => 60,
            "hour" => 3_600,
            "day" => 86_400,
            _ => return Err(format!("{name} has an invalid time unit")),
        };
        Ok(Self {
            count,
            window: Duration::from_secs(seconds),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub config_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub password_file: PathBuf,
    pub session_lifetime: Duration,
    pub max_request_bytes: usize,
    pub login_limit: RateLimit,
    pub diagnostic_action_limit: RateLimit,
    pub diagnostic_download_limit: RateLimit,
    pub reboot_limit: RateLimit,
    pub control_command: PathBuf,
    pub receiver_command: PathBuf,
    pub control_timeout: Duration,
    pub source_cache_ttl: Duration,
    pub source_target_file: PathBuf,
    pub video_ceiling_file: PathBuf,
    pub board_label: String,
    pub board_video_ceiling: String,
    pub playback_status_file: PathBuf,
    pub sdk_config_dir: PathBuf,
    pub runtime_config_file: PathBuf,
    pub playback_status_stale: Duration,
    pub diagnostics_host_report_file: PathBuf,
    pub diagnostics_host_request_file: PathBuf,
    pub diagnostics_host_pcap_file: PathBuf,
    pub diagnostics_host_pcap_metadata_file: PathBuf,
    pub diagnostics_host_timeout: Duration,
    pub diagnostics_host_budget: u64,
    pub diagnostics_bundle_budget: Duration,
    pub diagnostics_receive_probe: bool,
    pub version_file: PathBuf,
    pub runtime_integrity_manifest: PathBuf,
    pub project_license_file: PathBuf,
    pub third_party_notices_file: PathBuf,
    pub reboot_request_file: PathBuf,
    pub reboot_result_file: PathBuf,
    pub reboot_ack_timeout: Duration,
    pub web_port: u16,
    pub tls_cert_file: PathBuf,
    pub tls_key_file: PathBuf,
}

fn value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn path(name: &str, default: PathBuf) -> PathBuf {
    env::var_os(name).map_or(default, PathBuf::from)
}

fn integer(name: &str, default: u64, minimum: u64) -> Result<u64, String> {
    let raw = value(name, &default.to_string());
    raw.parse::<u64>()
        .ok()
        .filter(|number| *number >= minimum)
        .ok_or_else(|| format!("{name} must be an integer of at least {minimum}; received {raw:?}"))
}

fn seconds(name: &str, default: f64, allow_zero: bool) -> Result<Duration, String> {
    let raw = value(name, &default.to_string());
    let parsed = raw.parse::<f64>().ok().filter(|number| {
        number.is_finite()
            && if allow_zero {
                *number >= 0.0
            } else {
                *number > 0.0
            }
    });
    parsed
        .map(Duration::from_secs_f64)
        .ok_or_else(|| format!("{name} must be a finite positive number; received {raw:?}"))
}

fn boolean(name: &str, default: bool) -> Result<bool, String> {
    let raw = value(name, if default { "1" } else { "0" });
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("{name} must be a boolean; received {raw:?}")),
    }
}

impl Settings {
    pub fn load() -> Result<Self, String> {
        let mut obsolete: Vec<_> = env::vars()
            .map(|(name, _)| name)
            .filter(|name| {
                name == "PIPELINE_STATUS_STALE_SECONDS"
                    || name.starts_with("OMT_DEBUG_")
                    || name.starts_with("OMT_HOST_DEBUG_")
            })
            .collect();
        obsolete.sort();
        if !obsolete.is_empty() {
            return Err(format!(
                "Obsolete diagnostics settings are not supported: {}",
                obsolete.join(", ")
            ));
        }
        let config_dir = path("OMT_CONFIG_DIR", PathBuf::from("/etc/omt"));
        let runtime_dir = path("OMT_RUNTIME_DIR", config_dir.join("run"));
        let sdk_config_dir = path("OMT_STORAGE_PATH", config_dir.join("omt"));
        let runtime_config_file = path(
            "OMT_RUNTIME_CONFIG_FILE",
            sdk_config_dir.join("settings.xml"),
        );
        let host_timeout = seconds("OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS", 30.0, false)?;
        let bundle_budget = seconds("OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS", 60.0, false)?;
        if bundle_budget > Duration::from_secs(85) {
            return Err("OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS must be at most 85".to_owned());
        }
        if host_timeout > bundle_budget {
            return Err("OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS must not exceed OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS".to_owned());
        }
        let web_port_raw = value("WEB_PORT", "5000");
        let web_port = web_port_raw
            .parse()
            .map_err(|_| format!("WEB_PORT is invalid: {web_port_raw:?}"))?;
        let ssl_dir = config_dir.join("ssl");
        Ok(Self {
            password_file: path("OMT_PASSWORD_FILE", config_dir.join("web_password")),
            session_lifetime: Duration::from_secs(integer(
                "OMT_SESSION_LIFETIME_SECONDS",
                43_200,
                1,
            )?),
            max_request_bytes: usize::try_from(integer("OMT_MAX_REQUEST_BYTES", 16_384, 1_024)?)
                .map_err(|_| "OMT_MAX_REQUEST_BYTES is too large".to_owned())?,
            login_limit: RateLimit::parse("OMT_LOGIN_RATE_LIMIT", "5 per minute")?,
            diagnostic_download_limit: RateLimit::parse(
                "OMT_DIAGNOSTICS_DOWNLOAD_LIMIT",
                "10 per hour",
            )?,
            diagnostic_action_limit: RateLimit::parse(
                "OMT_DIAGNOSTICS_ACTION_LIMIT",
                "30 per hour",
            )?,
            reboot_limit: RateLimit::parse("OMT_REBOOT_ACTION_LIMIT", "3 per hour")?,
            control_command: path(
                "OMT_CONTROL_COMMAND",
                PathBuf::from("/usr/local/bin/control-omt.sh"),
            ),
            receiver_command: path(
                "OMT_RECEIVER_COMMAND",
                PathBuf::from("/usr/local/bin/omt-receiver"),
            ),
            control_timeout: seconds("OMT_CONTROL_TIMEOUT_SECONDS", 8.0, false)?,
            source_cache_ttl: seconds("OMT_SOURCE_CACHE_TTL_SECONDS", 5.0, true)?,
            source_target_file: path(
                "OMT_SOURCE_TARGET_FILE",
                config_dir.join("source_target.json"),
            ),
            video_ceiling_file: path(
                "OMT_VIDEO_CEILING_FILE",
                config_dir.join("video_ceiling.json"),
            ),
            board_label: value("OMT_BOARD_LABEL", "Raspberry Pi"),
            board_video_ceiling: value("OMT_VIDEO_CEILING", "1920x1080@60"),
            playback_status_file: path(
                "OMT_PLAYBACK_STATUS_FILE",
                runtime_dir.join("playback-status.json"),
            ),
            playback_status_stale: Duration::from_secs(integer(
                "OMT_PLAYBACK_STATUS_STALE_SECONDS",
                5,
                1,
            )?),
            diagnostics_host_report_file: path(
                "OMT_DIAGNOSTICS_HOST_REPORT_FILE",
                PathBuf::from("/host-diagnostics/host-report.txt"),
            ),
            diagnostics_host_request_file: path(
                "OMT_DIAGNOSTICS_HOST_REQUEST_FILE",
                PathBuf::from("/host-diagnostics/request"),
            ),
            diagnostics_host_pcap_file: path(
                "OMT_DIAGNOSTICS_HOST_PCAP_FILE",
                PathBuf::from("/host-diagnostics/host-network.pcap"),
            ),
            diagnostics_host_pcap_metadata_file: path(
                "OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE",
                PathBuf::from("/host-diagnostics/host-network-pcap.txt"),
            ),
            diagnostics_host_timeout: host_timeout,
            diagnostics_host_budget: integer("OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS", 25, 1)?,
            diagnostics_bundle_budget: bundle_budget,
            diagnostics_receive_probe: boolean("OMT_DIAGNOSTICS_RECEIVE_PROBE", true)?,
            version_file: path(
                "RPI_OMT_CLIENT_VERSION_FILE",
                PathBuf::from("/app/RPI_OMT_CLIENT_VERSION"),
            ),
            runtime_integrity_manifest: path(
                "OMT_RUNTIME_INTEGRITY_MANIFEST",
                PathBuf::from("/app/runtime-sha256.manifest"),
            ),
            project_license_file: path(
                "OMT_PROJECT_LICENSE_FILE",
                PathBuf::from("/app/legal/LICENSE"),
            ),
            third_party_notices_file: path(
                "OMT_THIRD_PARTY_NOTICES_FILE",
                PathBuf::from("/app/legal/THIRD_PARTY_NOTICES.txt"),
            ),
            reboot_request_file: path(
                "OMT_REBOOT_REQUEST_FILE",
                PathBuf::from("/host-actions/reboot.request"),
            ),
            reboot_result_file: path(
                "OMT_REBOOT_RESULT_FILE",
                PathBuf::from("/host-actions/reboot.result"),
            ),
            reboot_ack_timeout: seconds("OMT_REBOOT_ACK_TIMEOUT_SECONDS", 3.0, false)?,
            tls_cert_file: path("OMT_TLS_CERT_FILE", ssl_dir.join("cert.pem")),
            tls_key_file: path("OMT_TLS_KEY_FILE", ssl_dir.join("key.pem")),
            config_dir,
            runtime_dir,
            sdk_config_dir,
            runtime_config_file,
            web_port,
        })
    }

    pub fn diagnostic_lines(&self) -> Vec<String> {
        vec![
            format!(
                "session_lifetime_seconds={}",
                self.session_lifetime.as_secs()
            ),
            format!("max_request_bytes={}", self.max_request_bytes),
            format!(
                "control_timeout_seconds={}",
                self.control_timeout.as_secs_f64()
            ),
            format!(
                "source_cache_ttl_seconds={}",
                self.source_cache_ttl.as_secs_f64()
            ),
            format!(
                "playback_status_stale_seconds={}",
                self.playback_status_stale.as_secs()
            ),
            format!(
                "diagnostics_host_timeout_seconds={}",
                self.diagnostics_host_timeout.as_secs_f64()
            ),
            format!(
                "diagnostics_host_budget_seconds={}",
                self.diagnostics_host_budget
            ),
            format!(
                "diagnostics_bundle_budget_seconds={}",
                self.diagnostics_bundle_budget.as_secs_f64()
            ),
            format!(
                "diagnostics_receive_probe_enabled={}",
                self.diagnostics_receive_probe
            ),
            format!("board_label={}", self.board_label),
            format!("board_video_ceiling={}", self.board_video_ceiling),
            format!("sdk_config_dir={}", self.sdk_config_dir.display()),
            format!("runtime_config_file={}", self.runtime_config_file.display()),
            format!(
                "reboot_ack_timeout_seconds={}",
                self.reboot_ack_timeout.as_secs_f64()
            ),
        ]
    }
}
