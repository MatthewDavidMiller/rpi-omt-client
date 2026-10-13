//! The Web frontend's configuration, read once from the environment. Every
//! value is validated at startup; an invalid one stops the service with a
//! message naming the variable rather than falling back to a default.

use std::env;
use std::fmt::Write as _;

use omt_common::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimit {
    pub count: usize,
    pub window_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub config_dir: String,
    pub runtime_dir: String,
    pub password_file: String,
    pub session_lifetime_s: u64,
    pub max_request_bytes: usize,
    pub login_limit: RateLimit,
    pub diagnostic_action_limit: RateLimit,
    pub diagnostic_download_limit: RateLimit,
    pub reboot_limit: RateLimit,
    pub control_command: String,
    pub receiver_command: String,
    pub control_timeout_s: f64,
    pub source_cache_ttl_s: f64,
    pub source_target_file: String,
    pub video_ceiling_file: String,
    pub board_label: String,
    pub board_video_ceiling: String,
    pub playback_status_file: String,
    pub sdk_config_dir: String,
    pub runtime_config_file: String,
    pub playback_status_stale_s: u64,
    pub diagnostics_host_report_file: String,
    pub diagnostics_host_request_file: String,
    pub diagnostics_host_pcap_file: String,
    pub diagnostics_host_pcap_metadata_file: String,
    pub diagnostics_host_timeout_s: f64,
    pub diagnostics_host_budget: u64,
    pub diagnostics_bundle_budget_s: f64,
    pub diagnostics_receive_probe: bool,
    pub version_file: String,
    pub runtime_integrity_manifest: String,
    pub project_license_file: String,
    pub third_party_notices_file: String,
    pub reboot_request_file: String,
    pub reboot_result_file: String,
    pub reboot_ack_timeout_s: f64,
    pub web_port: u16,
    pub tls_cert_file: String,
    pub tls_key_file: String,
}

fn value(name: &str, default: &str) -> String {
    env::var_os(name).map_or_else(|| default.to_owned(), |v| v.to_string_lossy().into_owned())
}

/// u64::from_str: ASCII digits with an optional leading '+'.
fn parse_u64(raw: &str) -> Option<u64> {
    raw.parse().ok()
}

fn integer(name: &str, default: u64, minimum: u64) -> Result<u64> {
    let raw = value(name, &default.to_string());
    match parse_u64(&raw) {
        Some(v) if v >= minimum => Ok(v),
        _ => Err(Error::new(format!(
            "{name} must be an integer of at least {minimum}; received \"{raw}\""
        ))),
    }
}

fn seconds(name: &str, default: f64, allow_zero: bool) -> Result<f64> {
    let raw = value(name, &default.to_string());
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() && (if allow_zero { v >= 0.0 } else { v > 0.0 }) => Ok(v),
        _ => {
            Err(Error::new(format!("{name} must be a finite positive number; received \"{raw}\"")))
        }
    }
}

fn boolean(name: &str, default: bool) -> Result<bool> {
    let raw = value(name, if default { "1" } else { "0" });
    let t = raw.trim();
    for (yes, no) in [("1", "0"), ("true", "false"), ("yes", "no"), ("on", "off")] {
        if t.eq_ignore_ascii_case(yes) {
            return Ok(true);
        }
        if t.eq_ignore_ascii_case(no) {
            return Ok(false);
        }
    }
    Err(Error::new(format!("{name} must be a boolean; received \"{raw}\"")))
}

/// Parses "N per unit[s]".
pub fn parse_rate_limit(raw: &str) -> Option<RateLimit> {
    // Exactly three whitespace-separated words (C isspace, not Unicode).
    let words: Vec<&str> =
        raw.split([' ', '\t', '\n', '\r', '\x0c', '\x0b']).filter(|w| !w.is_empty()).collect();
    let [count, per, unit] = words.as_slice() else { return None };
    // The C parser bounded each word to 63 bytes.
    if words.iter().any(|w| w.len() >= 64) || *per != "per" {
        return None;
    }
    let count = parse_u64(count).filter(|&c| c != 0)?;
    let unit = match unit.trim_end_matches('s') {
        "second" => 1,
        "minute" => 60,
        "hour" => 3600,
        "day" => 86400,
        _ => return None,
    };
    Some(RateLimit { count: usize::try_from(count).ok()?, window_ms: unit * 1000 })
}

fn rate(name: &str, default: &str) -> Result<RateLimit> {
    let raw = value(name, default);
    parse_rate_limit(&raw).ok_or_else(|| {
        Error::new(format!("{name} must be a rate limit such as \"{default}\"; received \"{raw}\""))
    })
}

fn join(dir: &str, leaf: &str) -> String {
    format!("{dir}/{leaf}")
}

impl Settings {
    pub fn load() -> Result<Self> {
        // Retired diagnostics switches are refused by name, so a stale
        // compose file cannot silently run without what it asked for.
        let mut obsolete: Vec<String> = env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .filter(|k| {
                k == "PIPELINE_STATUS_STALE_SECONDS"
                    || k.starts_with("OMT_DEBUG_")
                    || k.starts_with("OMT_HOST_DEBUG_")
            })
            .collect();
        if !obsolete.is_empty() {
            obsolete.sort();
            return Err(Error::new(format!(
                "Obsolete diagnostics settings are not supported: {}",
                obsolete.join(", ")
            )));
        }
        let config_dir = value("OMT_CONFIG_DIR", "/etc/omt");
        let runtime_dir = value("OMT_RUNTIME_DIR", &join(&config_dir, "run"));
        let sdk_config_dir = value("OMT_STORAGE_PATH", &join(&config_dir, "omt"));
        let runtime_config_file =
            value("OMT_RUNTIME_CONFIG_FILE", &join(&sdk_config_dir, "settings.xml"));
        let diagnostics_host_timeout_s =
            seconds("OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS", 30.0, false)?;
        let diagnostics_bundle_budget_s =
            seconds("OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS", 60.0, false)?;
        if diagnostics_bundle_budget_s > 85.0 {
            return Err(Error::new("OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS must be at most 85"));
        }
        if diagnostics_host_timeout_s > diagnostics_bundle_budget_s {
            return Err(Error::new(
                "OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS must not exceed \
                 OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS",
            ));
        }
        let port_raw = value("WEB_PORT", "5000");
        let web_port = parse_u64(&port_raw)
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| Error::new(format!("WEB_PORT is invalid: \"{port_raw}\"")))?;
        let ssl = join(&config_dir, "ssl");
        let password_file = value("OMT_PASSWORD_FILE", &join(&config_dir, "web_password"));
        let session_lifetime_s = integer("OMT_SESSION_LIFETIME_SECONDS", 43200, 1)?;
        let max_request = integer("OMT_MAX_REQUEST_BYTES", 16384, 1024)?;
        Ok(Self {
            password_file,
            session_lifetime_s,
            max_request_bytes: usize::try_from(max_request).unwrap_or(usize::MAX),
            login_limit: rate("OMT_LOGIN_RATE_LIMIT", "5 per minute")?,
            diagnostic_download_limit: rate("OMT_DIAGNOSTICS_DOWNLOAD_LIMIT", "10 per hour")?,
            diagnostic_action_limit: rate("OMT_DIAGNOSTICS_ACTION_LIMIT", "30 per hour")?,
            reboot_limit: rate("OMT_REBOOT_ACTION_LIMIT", "3 per hour")?,
            control_command: value("OMT_CONTROL_COMMAND", "/usr/local/bin/control-omt.sh"),
            receiver_command: value("OMT_RECEIVER_COMMAND", "/usr/local/bin/omt-receiver"),
            control_timeout_s: seconds("OMT_CONTROL_TIMEOUT_SECONDS", 15.0, false)?,
            source_cache_ttl_s: seconds("OMT_SOURCE_CACHE_TTL_SECONDS", 5.0, true)?,
            source_target_file: value(
                "OMT_SOURCE_TARGET_FILE",
                &join(&config_dir, "source_target.json"),
            ),
            video_ceiling_file: value(
                "OMT_VIDEO_CEILING_FILE",
                &join(&config_dir, "video_ceiling.json"),
            ),
            board_label: value("OMT_BOARD_LABEL", "Raspberry Pi"),
            board_video_ceiling: value("OMT_VIDEO_CEILING", "1920x1080@60"),
            playback_status_file: value(
                "OMT_PLAYBACK_STATUS_FILE",
                &join(&runtime_dir, "playback-status.json"),
            ),
            playback_status_stale_s: integer("OMT_PLAYBACK_STATUS_STALE_SECONDS", 5, 1)?,
            diagnostics_host_report_file: value(
                "OMT_DIAGNOSTICS_HOST_REPORT_FILE",
                "/host-diagnostics/host-report.txt",
            ),
            diagnostics_host_request_file: value(
                "OMT_DIAGNOSTICS_HOST_REQUEST_FILE",
                "/host-diagnostics/request",
            ),
            diagnostics_host_pcap_file: value(
                "OMT_DIAGNOSTICS_HOST_PCAP_FILE",
                "/host-diagnostics/host-network.pcap",
            ),
            diagnostics_host_pcap_metadata_file: value(
                "OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE",
                "/host-diagnostics/host-network-pcap.txt",
            ),
            diagnostics_host_budget: integer("OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS", 25, 1)?,
            diagnostics_receive_probe: boolean("OMT_DIAGNOSTICS_RECEIVE_PROBE", true)?,
            version_file: value("RPI_OMT_CLIENT_VERSION_FILE", "/app/RPI_OMT_CLIENT_VERSION"),
            runtime_integrity_manifest: value(
                "OMT_RUNTIME_INTEGRITY_MANIFEST",
                "/app/runtime-sha256.manifest",
            ),
            project_license_file: value("OMT_PROJECT_LICENSE_FILE", "/app/legal/LICENSE"),
            third_party_notices_file: value(
                "OMT_THIRD_PARTY_NOTICES_FILE",
                "/app/legal/THIRD_PARTY_NOTICES.txt",
            ),
            reboot_request_file: value("OMT_REBOOT_REQUEST_FILE", "/host-actions/reboot.request"),
            reboot_result_file: value("OMT_REBOOT_RESULT_FILE", "/host-actions/reboot.result"),
            reboot_ack_timeout_s: seconds("OMT_REBOOT_ACK_TIMEOUT_SECONDS", 3.0, false)?,
            web_port,
            tls_cert_file: value("OMT_TLS_CERT_FILE", &join(&ssl, "cert.pem")),
            tls_key_file: value("OMT_TLS_KEY_FILE", &join(&ssl, "key.pem")),
            config_dir,
            runtime_dir,
            sdk_config_dir,
            runtime_config_file,
            diagnostics_host_timeout_s,
            diagnostics_bundle_budget_s,
        })
    }

    /// The settings lines the support bundle records (runtime-settings.txt).
    pub fn diagnostic_lines(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "session_lifetime_seconds={}", self.session_lifetime_s);
        let _ = writeln!(out, "max_request_bytes={}", self.max_request_bytes);
        let _ = writeln!(out, "control_timeout_seconds={}", self.control_timeout_s);
        let _ = writeln!(out, "source_cache_ttl_seconds={}", self.source_cache_ttl_s);
        let _ = writeln!(out, "playback_status_stale_seconds={}", self.playback_status_stale_s);
        let _ =
            writeln!(out, "diagnostics_host_timeout_seconds={}", self.diagnostics_host_timeout_s);
        let _ = writeln!(out, "diagnostics_host_budget_seconds={}", self.diagnostics_host_budget);
        let _ =
            writeln!(out, "diagnostics_bundle_budget_seconds={}", self.diagnostics_bundle_budget_s);
        let _ =
            writeln!(out, "diagnostics_receive_probe_enabled={}", self.diagnostics_receive_probe);
        let _ = writeln!(out, "board_label={}", self.board_label);
        let _ = writeln!(out, "board_video_ceiling={}", self.board_video_ceiling);
        let _ = writeln!(out, "sdk_config_dir={}", self.sdk_config_dir);
        let _ = writeln!(out, "runtime_config_file={}", self.runtime_config_file);
        let _ = writeln!(out, "reboot_ack_timeout_seconds={}", self.reboot_ack_timeout_s);
        out
    }
}
