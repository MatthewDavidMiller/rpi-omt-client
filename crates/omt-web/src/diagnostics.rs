use crate::{
    command::{CommandResult, run},
    io::{read_bounded, read_text, write_fixed_inode},
    playback::Playback,
    settings::Settings,
};
use omt_protocol::parse_direct_target;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};
use time::OffsetDateTime;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// Host capture and the in-memory ZIP share this ceiling so a support download
/// cannot push the appliance over its container memory cap.
const PCAP_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticResult {
    pub title: String,
    pub command: CommandResult,
}

pub struct Bundle {
    pub bytes: Vec<u8>,
    pub filename: String,
}

pub struct Diagnostics {
    settings: Settings,
    playback: Arc<Playback>,
    collection_lock: Mutex<()>,
    reboot_lock: Mutex<()>,
}

impl Diagnostics {
    pub fn new(settings: &Settings, playback: Arc<Playback>) -> Self {
        Self {
            settings: settings.clone(),
            playback,
            collection_lock: Mutex::new(()),
            reboot_lock: Mutex::new(()),
        }
    }

    pub fn status(&self) -> String {
        run(
            &self.settings.control_command,
            &["status"],
            self.settings.control_timeout,
        )
        .report_text()
    }

    pub fn discovery(&self) -> DiagnosticResult {
        let mut result = run(
            &self.settings.receiver_command,
            &["discover", "--wait-ms", "3000", "--json"],
            Duration::from_secs(5),
        );
        if result.returncode == Some(0) {
            result.sources = serde_json::from_str::<Vec<serde_json::Value>>(&result.stdout)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("name")?.as_str().map(ToOwned::to_owned))
                .collect();
        }
        DiagnosticResult {
            title: "OMT discovery check".to_owned(),
            command: result,
        }
    }

    pub fn runtime(&self) -> (DiagnosticResult, String) {
        let version = run(
            &self.settings.receiver_command,
            &["--version"],
            Duration::from_secs(3),
        );
        let status = run(
            &self.settings.control_command,
            &["status"],
            Duration::from_secs(3),
        );
        let output = format!(
            "$ {}\n{}{}{}\n\n$ {}\n{}{}{}",
            version.command,
            version.stdout,
            version.stderr,
            version.error,
            status.command,
            status.stdout,
            status.stderr,
            status.error
        );
        let ok = version.returncode == Some(0) && matches!(status.returncode, Some(0 | 3));
        let report = CommandResult {
            command: "OMT runtime checks".to_owned(),
            returncode: Some(i32::from(!ok)),
            stdout: output,
            error: if ok {
                String::new()
            } else {
                "One or more runtime checks failed.".to_owned()
            },
            duration_seconds: version.duration_seconds + status.duration_seconds,
            ..CommandResult::default()
        };
        (
            DiagnosticResult {
                title: "Runtime check".to_owned(),
                command: report,
            },
            status.report_text(),
        )
    }

    pub fn direct(&self, address: &str) -> DiagnosticResult {
        if parse_direct_target(address).is_err() {
            return DiagnosticResult {
                title: "Direct-connect check".to_owned(),
                command: CommandResult {
                    error: "Invalid OMT direct target.".to_owned(),
                    skipped: true,
                    ..CommandResult::default()
                },
            };
        }
        DiagnosticResult {
            title: "Direct-connect check".to_owned(),
            command: run(
                &self.settings.receiver_command,
                &[
                    "probe",
                    "--target",
                    address,
                    "--timeout-ms",
                    "3000",
                    "--json",
                ],
                Duration::from_secs(5),
            ),
        }
    }

    pub fn bundle(&self, include_pcap: bool, version: &str) -> Result<Bundle, String> {
        let _guard = self
            .collection_lock
            .lock()
            .map_err(|_| "diagnostic collection lock failed")?;
        let deadline = Instant::now() + self.settings.diagnostics_bundle_budget;
        let request_id = crate::io::random_hex(16)?;
        let request = format!(
            "version=1\nrequest_id={request_id}\ncapture_pcap={}\nrequested_at_epoch={}\n",
            u8::from(include_pcap),
            now_epoch()
        );
        let request_error = write_fixed_inode(
            &self.settings.diagnostics_host_request_file,
            request.as_bytes(),
            512,
        )
        .err();
        let (runtime_result, controller_status) = self.runtime();
        let discovery = self.discovery().command;
        let configuration = self.playback.configuration();
        let receive_probe = if self.settings.diagnostics_receive_probe && configuration.configured()
        {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            run(
                &self.settings.receiver_command,
                &[
                    "probe",
                    "--target",
                    &configuration.source,
                    "--timeout-ms",
                    "3000",
                    "--json",
                ],
                remaining.max(Duration::from_millis(1)),
            )
            .stdout
        } else {
            json_error("skipped: no current target or receive probe disabled")
        };
        let host_report = if let Some(error) = request_error {
            unavailable(&format!(
                "unable to submit host diagnostic request: {error}"
            ))
        } else {
            self.wait_for_host_report(&request_id, deadline)
        };
        let capture = if include_pcap {
            self.capture(&request_id)?
        } else {
            Capture::default()
        };

        let cursor = Cursor::new(Vec::new());
        let mut archive = ZipWriter::new(cursor);
        let compressed =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        write_member(
            &mut archive,
            "version.txt",
            format!("{version}\n").as_bytes(),
            compressed,
        )?;
        write_member(
            &mut archive,
            "runtime-settings.txt",
            format!("{}\n", self.settings.diagnostic_lines().join("\n")).as_bytes(),
            compressed,
        )?;
        write_member(
            &mut archive,
            "runtime.txt",
            runtime_result.command.stdout.as_bytes(),
            compressed,
        )?;
        let discovery_member = if discovery.returncode == Some(0)
            && serde_json::from_str::<Vec<serde_json::Value>>(&discovery.stdout).is_ok()
        {
            discovery.stdout
        } else {
            json_error(discovery.failure_detail())
        };
        write_member(
            &mut archive,
            "discovery.json",
            discovery_member.as_bytes(),
            compressed,
        )?;
        write_member(
            &mut archive,
            "controller-status.txt",
            format!("{controller_status}\n").as_bytes(),
            compressed,
        )?;
        let receive_member = if serde_json::from_str::<serde_json::Value>(&receive_probe).is_ok() {
            receive_probe
        } else {
            json_error(&receive_probe)
        };
        write_member(
            &mut archive,
            "current-target-receive-probe.json",
            receive_member.as_bytes(),
            compressed,
        )?;
        for (name, path, limit) in [
            (
                "playback-status.json",
                &self.settings.playback_status_file,
                4_096,
            ),
            (
                "omt-settings.xml",
                &self.settings.runtime_config_file,
                65_536,
            ),
            (
                "runtime-sha256.manifest",
                &self.settings.runtime_integrity_manifest,
                262_144,
            ),
        ] {
            let data = read_bounded(path, limit)
                .ok()
                .flatten()
                .unwrap_or_else(|| unavailable("file unavailable"));
            write_member(&mut archive, name, &data, compressed)?;
        }
        write_member(&mut archive, "host-report.txt", &host_report, compressed)?;
        write_member(
            &mut archive,
            "host-network-pcap.txt",
            &capture.metadata,
            compressed,
        )?;
        if let Some(data) = capture.data {
            write_member(&mut archive, "host-network.pcap", &data, stored)?;
        } else if include_pcap {
            write_member(
                &mut archive,
                "host-network.pcap.unavailable.txt",
                &unavailable(&capture.error),
                compressed,
            )?;
        }
        let output = archive
            .finish()
            .map_err(|error| error.to_string())?
            .into_inner();
        let timestamp = OffsetDateTime::from(SystemTime::now())
            .format(&time::macros::format_description!(
                "[year][month][day]T[hour][minute][second]Z"
            ))
            .map_err(|error| error.to_string())?;
        Ok(Bundle {
            bytes: output,
            filename: format!("omt-diagnostics-{timestamp}.zip"),
        })
    }

    pub fn request_reboot(&self) -> crate::playback::ActionResult {
        let Ok(_guard) = self.reboot_lock.lock() else {
            return crate::playback::ActionResult {
                ok: false,
                message: String::new(),
                error: "Host reboot request lock failed.".to_owned(),
            };
        };
        request_reboot(&self.settings)
    }

    fn wait_for_host_report(&self, request_id: &str, deadline: Instant) -> Vec<u8> {
        let end = deadline.min(Instant::now() + self.settings.diagnostics_host_timeout);
        let mut detail = "host diagnostic report was not published".to_owned();
        while Instant::now() < end {
            match read_text(
                &self.settings.diagnostics_host_report_file,
                16 * 1024 * 1024,
            ) {
                Ok(Some(report)) => {
                    if let Some(fields) =
                        parse_record(&report, &["version", "request_id", "status"], true)
                        && fields.get("version").is_some_and(|value| value == "1")
                        && fields
                            .get("request_id")
                            .is_some_and(|value| value == request_id)
                        && fields
                            .get("status")
                            .is_some_and(|value| matches!(value.as_str(), "complete" | "partial"))
                    {
                        return report.into_bytes();
                    }
                    "host diagnostic report did not match this request".clone_into(&mut detail);
                }
                Ok(None) => {}
                Err(error) => detail = error,
            }
            thread::sleep(Duration::from_millis(50));
        }
        unavailable(&detail)
    }

    fn capture(&self, request_id: &str) -> Result<Capture, String> {
        let metadata = read_text(
            &self.settings.diagnostics_host_pcap_metadata_file,
            64 * 1024,
        )?
        .unwrap_or_default();
        let required = [
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
        let Some(fields) = parse_record(&metadata, &required, true) else {
            return Ok(Capture {
                metadata: metadata.into_bytes(),
                error: "capture metadata schema is invalid".to_owned(),
                data: None,
            });
        };
        if fields.get("version").map(String::as_str) != Some("1")
            || fields.get("request_id").map(String::as_str) != Some(request_id)
        {
            return Ok(Capture {
                metadata: metadata.into_bytes(),
                error: "capture metadata does not match this request".to_owned(),
                data: None,
            });
        }
        let complete = fields.get("capture_status").is_some_and(|value| {
            matches!(value.as_str(), "complete" | "time_limit" | "size_limit")
        });
        if !complete {
            return Ok(Capture {
                metadata: metadata.into_bytes(),
                error: fields
                    .get("capture_status")
                    .cloned()
                    .unwrap_or_else(|| "capture unavailable".to_owned()),
                data: None,
            });
        }
        let expected_max = PCAP_MAX_BYTES.to_string();
        if fields.get("max_bytes").map(String::as_str) != Some(expected_max.as_str()) {
            return Err("capture metadata size exceeds the limit".to_owned());
        }
        let expected_size = fields
            .get("size_bytes")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|size| *size <= PCAP_MAX_BYTES)
            .ok_or("capture metadata size is invalid")?;
        let data = read_bounded(&self.settings.diagnostics_host_pcap_file, PCAP_MAX_BYTES)?
            .ok_or("packet capture is missing")?;
        if data.len() != expected_size || data.len() < 4 {
            return Err("packet capture has an unexpected size".to_owned());
        }
        let allowed_magic = [
            [0xd4, 0xc3, 0xb2, 0xa1],
            [0xa1, 0xb2, 0xc3, 0xd4],
            [0x4d, 0x3c, 0xb2, 0xa1],
            [0xa1, 0xb2, 0x3c, 0x4d],
            [0x0a, 0x0d, 0x0d, 0x0a],
        ];
        if !allowed_magic.contains(&data[..4].try_into().unwrap_or([0; 4])) {
            return Err("packet capture magic is invalid".to_owned());
        }
        let digest = format!("{:x}", Sha256::digest(&data));
        if fields.get("sha256") != Some(&digest) {
            return Err("packet capture SHA-256 does not match metadata".to_owned());
        }
        Ok(Capture {
            metadata: metadata.into_bytes(),
            error: String::new(),
            data: Some(data),
        })
    }
}

#[derive(Default)]
struct Capture {
    metadata: Vec<u8>,
    error: String,
    data: Option<Vec<u8>>,
}

fn write_member(
    archive: &mut ZipWriter<Cursor<Vec<u8>>>,
    name: &str,
    data: &[u8],
    options: SimpleFileOptions,
) -> Result<(), String> {
    archive
        .start_file(name, options)
        .map_err(|error| error.to_string())?;
    archive.write_all(data).map_err(|error| error.to_string())
}

fn unavailable(detail: &str) -> Vec<u8> {
    format!("unavailable: {detail}\n").into_bytes()
}
fn json_error(detail: &str) -> String {
    serde_json::json!({"ok": false, "error": if detail.trim().is_empty() { "command failed" } else { detail.trim() }}).to_string()
}
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

fn parse_record(
    value: &str,
    required: &[&str],
    allow_body: bool,
) -> Option<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for line in value.lines() {
        if line.is_empty() && allow_body {
            break;
        }
        let (key, field) = line.split_once('=')?;
        if key.is_empty() || fields.insert(key.to_owned(), field.to_owned()).is_some() {
            return None;
        }
    }
    (fields.len() == required.len() && required.iter().all(|key| fields.contains_key(*key)))
        .then_some(fields)
}

pub fn version(settings: &Settings) -> String {
    read_text(&settings.version_file, 256)
        .ok()
        .flatten()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

pub fn legal_texts(settings: &Settings) -> (String, String) {
    let license = read_text(&settings.project_license_file, 1024 * 1024)
        .ok()
        .flatten()
        .unwrap_or_else(|| "License text unavailable.".to_owned());
    let notices = read_text(&settings.third_party_notices_file, 8 * 1024 * 1024)
        .ok()
        .flatten()
        .unwrap_or_else(|| "Third-party notices unavailable.".to_owned());
    (license, notices)
}

fn request_reboot(settings: &Settings) -> crate::playback::ActionResult {
    let Ok(request_id) = crate::io::random_hex(16) else {
        return crate::playback::ActionResult {
            ok: false,
            message: String::new(),
            error: "Unable to generate reboot request ID.".to_owned(),
        };
    };
    let record = format!(
        "version=1\naction=reboot\nrequest_id={request_id}\nrequested_at_epoch={}\n",
        now_epoch()
    );
    if let Err(error) = write_fixed_inode(&settings.reboot_request_file, record.as_bytes(), 512) {
        return crate::playback::ActionResult {
            ok: false,
            message: String::new(),
            error: format!("Unable to submit the host reboot request: {error}"),
        };
    }
    let deadline = Instant::now() + settings.reboot_ack_timeout;
    while Instant::now() < deadline {
        if let Ok(Some(result)) = read_text(&settings.reboot_result_file, 512)
            && let Some(fields) = parse_record(
                &result,
                &["version", "request_id", "status", "detail"],
                false,
            )
            && fields.get("version").map(String::as_str) == Some("1")
            && fields.get("request_id").map(String::as_str) == Some(request_id.as_str())
        {
            if fields.get("status").map(String::as_str) == Some("accepted") {
                return crate::playback::ActionResult {
                    ok: true,
                    message: "OS reboot scheduled. This appliance will go offline shortly."
                        .to_owned(),
                    error: String::new(),
                };
            }
            if fields.get("status").map(String::as_str) == Some("rejected") {
                return crate::playback::ActionResult {
                    ok: false,
                    message: String::new(),
                    error: format!(
                        "The host rejected the reboot request: {}",
                        fields.get("detail").map_or("unknown", String::as_str)
                    ),
                };
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    crate::playback::ActionResult { ok: false, message: String::new(), error: "The reboot request was submitted but the host did not acknowledge it. Check rc-service omt-client-reboot status and /var/log/messages before retrying.".to_owned() }
}
