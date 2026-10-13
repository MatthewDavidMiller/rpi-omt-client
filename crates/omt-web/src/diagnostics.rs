//! Bounded diagnostics: the discovery, runtime, and direct-connect checks,
//! the support bundle (docs/DIAGNOSTICS_BUNDLE.md), and the host reboot
//! request.

use std::thread;
use std::time::Duration;

use omt_common::timefmt::{compact_utc, now_ms, remaining_ms, unix_seconds};
use omt_common::{Error, Result, fsio, json, proc, rand};
use omt_protocol::{SOURCE_NAME_MAX_BYTES, parse_direct_target};

use crate::playback::{ActionResult, MAX_CHOICES, Playback, control, ms_of};
use crate::settings::Settings;
use crate::zip::{Method, Zip};

/// Host capture and the in-memory ZIP share this ceiling so a support
/// download cannot push the appliance over its container memory cap.
pub const PCAP_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct DiagnosticResult {
    pub title: &'static str,
    pub command: proc::Output,
    pub skipped: bool,
    pub sources: Vec<String>,
}

fn now_epoch() -> u64 {
    u64::try_from(unix_seconds()).unwrap_or(0)
}

fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

fn run(argv: &[&str], timeout_ms: u64) -> proc::Output {
    proc::run(&proc::Options::new(argv, timeout_ms))
}

/// control-omt.sh status, as report text.
pub fn status(s: &Settings) -> String {
    control(s, "status", s.control_timeout_s).report_text().to_owned()
}

pub fn discovery(s: &Settings) -> DiagnosticResult {
    let argv = [s.receiver_command.as_str(), "discover", "--wait-ms", "3000", "--json"];
    let mut out = DiagnosticResult {
        title: "OMT discovery check",
        command: run(&argv, 5000),
        ..Default::default()
    };
    if !out.command.succeeded() {
        return out;
    }
    // The names of a well-formed answer, unfiltered: this check reports what
    // the receiver said.
    if let Ok(root) = json::parse(out.command.stdout.as_bytes())
        && let Some(items) = root.as_array()
    {
        out.sources = items
            .iter()
            .filter_map(|e| e.get("name").and_then(json::Value::as_str))
            .filter(|n| n.len() <= SOURCE_NAME_MAX_BYTES)
            .take(MAX_CHOICES)
            .map(str::to_owned)
            .collect();
    }
    out
}

/// The runtime check, and the controller status it observed.
pub fn runtime(s: &Settings) -> (DiagnosticResult, String) {
    let version = run(&[&s.receiver_command, "--version"], 3000);
    let status = run(&[&s.control_command, "status"], 3000);
    let ok = version.succeeded() && matches!(status.returncode, Some(0 | 3));
    let command = proc::Output {
        command: "OMT runtime checks".to_owned(),
        returncode: Some(i32::from(!ok)),
        stdout: format!(
            "$ {}\n{}{}{}\n\n$ {}\n{}{}{}",
            version.command,
            version.stdout,
            version.stderr,
            version.error,
            status.command,
            status.stdout,
            status.stderr,
            status.error
        ),
        error: if ok { String::new() } else { "One or more runtime checks failed.".to_owned() },
        duration_seconds: version.duration_seconds + status.duration_seconds,
        ..proc::Output::default()
    };
    let status_text = status.report_text().to_owned();
    (DiagnosticResult { title: "Runtime check", command, ..Default::default() }, status_text)
}

pub fn direct(s: &Settings, address: &str) -> DiagnosticResult {
    if parse_direct_target(address).is_err() {
        return DiagnosticResult {
            title: "Direct-connect check",
            command: proc::Output {
                error: "Invalid OMT direct target.".to_owned(),
                ..proc::Output::default()
            },
            skipped: true,
            sources: Vec::new(),
        };
    }
    let argv = [
        s.receiver_command.as_str(),
        "probe",
        "--target",
        address,
        "--timeout-ms",
        "3000",
        "--json",
    ];
    DiagnosticResult {
        title: "Direct-connect check",
        command: run(&argv, 5000),
        ..Default::default()
    }
}

/// Parses a key=value record: every key exactly once, exactly `required`,
/// and (with `allow_body`) stopping at the first empty line. The values come
/// back in `required` order.
pub fn parse_record<'a>(
    text: &'a str,
    required: &[&str],
    allow_body: bool,
) -> Option<Vec<&'a str>> {
    let mut values: Vec<Option<&str>> = vec![None; required.len()];
    // str::lines: '\n' separated, a trailing "\r" stripped, no empty last.
    for line in text.lines() {
        if line.is_empty() && allow_body {
            break;
        }
        let (key, value) = line.split_once('=').filter(|(k, _)| !k.is_empty())?;
        // An unexpected key makes the field count wrong.
        let i = required.iter().position(|r| *r == key)?;
        if values[i].replace(value).is_some() {
            return None;
        }
    }
    values.into_iter().collect()
}

fn unavailable(detail: &str) -> String {
    format!("unavailable: {detail}\n")
}

fn json_error(detail: &str) -> String {
    let t = detail.trim();
    let mut out = String::from("{\"error\":");
    json::write_string(&mut out, if t.is_empty() { "command failed" } else { t });
    out.push_str(",\"ok\":false}");
    out
}

fn wait_for_host_report(s: &Settings, request_id: &str, deadline_ms: u64) -> String {
    let end = deadline_ms.min(now_ms() + ms_of(s.diagnostics_host_timeout_s));
    let mut detail = "host diagnostic report was not published".to_owned();
    while now_ms() < end {
        match fsio::read_text(&s.diagnostics_host_report_file, 16 * 1024 * 1024) {
            Ok(Some(report)) => {
                let fields = ["version", "request_id", "status"];
                if let Some(v) = parse_record(&report, &fields, true)
                    && v[0] == "1"
                    && v[1] == request_id
                    && (v[2] == "complete" || v[2] == "partial")
                {
                    return report;
                }
                "host diagnostic report did not match this request".clone_into(&mut detail);
            }
            Ok(None) => {}
            Err(e) => detail = e.to_string(),
        }
        sleep_ms(50);
    }
    unavailable(&detail)
}

/// The host's packet capture: `Ok(Ok(bytes))` with data, `Ok(Err(reason))`
/// when the capture is unavailable, and `Err` for a capture that breaks the
/// contract, which fails the whole bundle.
const MAGIC: [[u8; 4]; 5] = [
    [0xd4, 0xc3, 0xb2, 0xa1],
    [0xa1, 0xb2, 0xc3, 0xd4],
    [0x4d, 0x3c, 0xb2, 0xa1],
    [0xa1, 0xb2, 0x3c, 0x4d],
    [0x0a, 0x0d, 0x0d, 0x0a],
];

fn capture(
    s: &Settings,
    request_id: &str,
    metadata: &mut String,
) -> Result<std::result::Result<Vec<u8>, String>> {
    let meta = fsio::read_text(&s.diagnostics_host_pcap_metadata_file, 64 * 1024)?;
    if let Some(m) = &meta {
        metadata.push_str(m);
    }
    let fields = [
        "version",
        "request_id",
        "capture_status",
        "capture_interface",
        "capture_filter",
        "capture_snaplen",
        "capture_seconds",
        "max_bytes",
        "size_bytes",
        "sha256",
        "pcap_magic",
        "tcpdump_exit_status",
    ];
    let Some(v) = meta.as_deref().and_then(|m| parse_record(m, &fields, true)) else {
        return Ok(Err("capture metadata schema is invalid".to_owned()));
    };
    if v[0] != "1" || v[1] != request_id {
        return Ok(Err("capture metadata does not match this request".to_owned()));
    }
    if !matches!(v[2], "complete" | "time_limit" | "size_limit") {
        return Ok(Err(v[2].to_owned()));
    }
    if v[7] != PCAP_MAX_BYTES.to_string() {
        return Err(Error::new("capture metadata size exceeds the limit"));
    }
    let size = omt_common::buf::parse_u64(v[8].as_bytes(), PCAP_MAX_BYTES as u64)
        .ok_or_else(|| Error::new("capture metadata size is invalid"))?;
    let pcap = fsio::read_bounded(&s.diagnostics_host_pcap_file, PCAP_MAX_BYTES)?
        .ok_or_else(|| Error::new("packet capture is missing"))?;
    if pcap.len() as u64 != size || pcap.len() < 4 {
        return Err(Error::new("packet capture has an unexpected size"));
    }
    if !MAGIC.iter().any(|m| pcap.starts_with(m)) {
        return Err(Error::new("packet capture magic is invalid"));
    }
    match omt_crypto::sha256_hex(&pcap) {
        Ok(digest) if digest == v[9] => Ok(Ok(pcap)),
        _ => Err(Error::new("packet capture SHA-256 does not match metadata")),
    }
}

fn valid_json(text: &str, require_array: bool) -> bool {
    json::parse(text.as_bytes()).is_ok_and(|root| !require_array || root.as_array().is_some())
}

/// Builds the support bundle; returns the archive and its file name.
pub fn bundle(
    s: &Settings,
    playback: &Playback,
    include_pcap: bool,
    version: &str,
) -> Result<(Vec<u8>, String)> {
    let deadline = now_ms() + ms_of(s.diagnostics_bundle_budget_s);
    let request_id = rand::random_hex(16)?;
    let request = format!(
        "version=1\nrequest_id={request_id}\ncapture_pcap={}\nrequested_at_epoch={}\n",
        u8::from(include_pcap),
        now_epoch()
    );
    let request_ok =
        fsio::write_fixed_inode(&s.diagnostics_host_request_file, request.as_bytes(), 512);

    let (runtime, controller) = runtime(s);
    let discovery = discovery(s);
    let configuration = playback.configuration();
    let receive = if s.diagnostics_receive_probe && configuration.configured() {
        let left = remaining_ms(deadline).clamp(1, 5000);
        let argv = [
            s.receiver_command.as_str(),
            "probe",
            "--target",
            &configuration.source,
            "--timeout-ms",
            "3000",
            "--json",
        ];
        run(&argv, left).stdout
    } else {
        json_error("skipped: no current target or receive probe disabled")
    };
    let host = match request_ok {
        Err(e) => unavailable(&format!("unable to submit host diagnostic request: {e}")),
        Ok(()) => wait_for_host_report(s, &request_id, deadline),
    };
    let mut pcap_meta = String::new();
    let captured = if include_pcap { Some(capture(s, &request_id, &mut pcap_meta)?) } else { None };

    let mut z = Zip::new(48 * 1024 * 1024);
    z.add("version.txt", format!("{version}\n").as_bytes(), Method::Deflated);
    z.add("runtime-settings.txt", s.diagnostic_lines().as_bytes(), Method::Deflated);
    z.add("runtime.txt", runtime.command.stdout.as_bytes(), Method::Deflated);
    let discovered = if discovery.command.succeeded() && valid_json(&discovery.command.stdout, true)
    {
        discovery.command.stdout.clone()
    } else {
        json_error(discovery.command.failure_detail())
    };
    z.add("discovery.json", discovered.as_bytes(), Method::Deflated);
    z.add("controller-status.txt", format!("{controller}\n").as_bytes(), Method::Deflated);
    let probe = if valid_json(&receive, false) { receive } else { json_error(&receive) };
    z.add("current-target-receive-probe.json", probe.as_bytes(), Method::Deflated);
    for (name, path, limit) in [
        ("playback-status.json", &s.playback_status_file, 4096),
        ("omt-settings.xml", &s.runtime_config_file, 65536),
        ("runtime-sha256.manifest", &s.runtime_integrity_manifest, 262_144),
    ] {
        match fsio::read_bounded(path, limit) {
            Ok(Some(file)) => z.add(name, &file, Method::Deflated),
            _ => z.add(name, unavailable("file unavailable").as_bytes(), Method::Deflated),
        }
    }
    z.add("host-report.txt", host.as_bytes(), Method::Deflated);
    z.add("host-network-pcap.txt", pcap_meta.as_bytes(), Method::Deflated);
    match &captured {
        Some(Ok(pcap)) => z.add("host-network.pcap", pcap, Method::Stored),
        Some(Err(reason)) => z.add(
            "host-network.pcap.unavailable.txt",
            unavailable(reason).as_bytes(),
            Method::Deflated,
        ),
        None => {}
    }
    let archive = z.finish().ok_or_else(|| Error::new("unable to assemble the support bundle"))?;
    let filename = format!("omt-diagnostics-{}.zip", compact_utc(unix_seconds()));
    Ok((archive, filename))
}

pub fn request_reboot(s: &Settings) -> ActionResult {
    let Ok(id) = rand::random_hex(16) else {
        return ActionResult::failure("Unable to generate reboot request ID.");
    };
    let record =
        format!("version=1\naction=reboot\nrequest_id={id}\nrequested_at_epoch={}\n", now_epoch());
    if let Err(e) = fsio::write_fixed_inode(&s.reboot_request_file, record.as_bytes(), 512) {
        return ActionResult::failure(format!("Unable to submit the host reboot request: {e}"));
    }
    let deadline = now_ms() + ms_of(s.reboot_ack_timeout_s);
    while now_ms() < deadline {
        if let Ok(Some(result)) = fsio::read_text(&s.reboot_result_file, 512) {
            let fields = ["version", "request_id", "status", "detail"];
            if let Some(v) = parse_record(&result, &fields, false)
                && v[0] == "1"
                && v[1] == id
            {
                if v[2] == "accepted" {
                    return ActionResult::success(
                        "OS reboot scheduled. This appliance will go offline shortly.",
                    );
                }
                if v[2] == "rejected" {
                    return ActionResult::failure(format!(
                        "The host rejected the reboot request: {}",
                        v[3]
                    ));
                }
            }
        }
        sleep_ms(50);
    }
    ActionResult::failure(
        "The reboot request was submitted but the host did not acknowledge it. Check rc-service \
         omt-client-reboot status and /var/log/messages before retrying.",
    )
}

/// The appliance version (from the version file, else the build's own).
pub fn app_version(s: &Settings) -> String {
    if let Ok(Some(text)) = fsio::read_text(&s.version_file, 256) {
        let t = text.trim();
        if !t.is_empty() {
            return t.to_owned();
        }
    }
    // The Rust frontend fell back to its crate version, which never carried
    // the leading 'v' the build stamps on.
    let v = omt_common::VERSION;
    v.strip_prefix('v').unwrap_or(v).to_owned()
}

/// The project license and the third-party notices.
pub fn legal_texts(s: &Settings) -> (String, String) {
    let license = match fsio::read_text(&s.project_license_file, 1024 * 1024) {
        Ok(Some(t)) => t,
        _ => "License text unavailable.".to_owned(),
    };
    let notices = match fsio::read_text(&s.third_party_notices_file, 8 * 1024 * 1024) {
        Ok(Some(t)) => t,
        _ => "Third-party notices unavailable.".to_owned(),
    };
    (license, notices)
}
