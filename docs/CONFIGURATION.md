# Configuration

## Environment

| Variable | Default / purpose |
|---|---|
| `WEB_PORT` | `5000`, HTTPS listener |
| `RPI_OMT_CLIENT_VERSION` | Build version embedded in image/receiver |
| `RPI_OMT_CLIENT_VERSION_FILE` | `/app/RPI_OMT_CLIENT_VERSION` |
| `OMT_CONFIG_DIR` | `/etc/omt` persistent application state |
| `OMT_RUNTIME_DIR` | `/run/omt/state` in the shipped image (tmpfs); per-boot lock, PID record, and playback status. Falls back to `$OMT_CONFIG_DIR/run` when unset |
| `OMT_STORAGE_PATH` | `/etc/omt/omt`, native OMT settings storage |
| `OMT_RUNTIME_CONFIG_FILE` | `$OMT_STORAGE_PATH/settings.xml` |
| `OMT_PASSWORD_FILE` | `$OMT_CONFIG_DIR/web_password` |
| `OMT_WEB_PASSWORD` | Emergency plaintext password override (unset in production). When set, the password file is ignored and the value is compared in constant time. Prefer a hashed `web_password` file. |
| `OMT_TLS_CERT_FILE` | `$OMT_CONFIG_DIR/ssl/cert.pem` |
| `OMT_TLS_KEY_FILE` | `$OMT_CONFIG_DIR/ssl/key.pem` |
| `OMT_SESSION_LIFETIME_SECONDS` | `43200` |
| `OMT_MAX_REQUEST_BYTES` | `16384` request body ceiling, minimum `1024` |
| `OMT_LOGIN_RATE_LIMIT` | `5 per minute` |
| `OMT_RECEIVER_COMMAND` | `/usr/local/bin/omt-receiver` |
| `OMT_CONTROL_COMMAND` | `/usr/local/bin/control-omt.sh` |
| `OMT_CONTROL_TIMEOUT_SECONDS` | `8` |
| `OMT_SOURCE_CACHE_TTL_SECONDS` | `5` |
| `OMT_SOURCE_TARGET_FILE` | `$OMT_CONFIG_DIR/source_target.json` |
| `OMT_PLAYBACK_STATUS_FILE` | `$OMT_RUNTIME_DIR/playback-status.json` |
| `OMT_PLAYBACK_STATUS_STALE_SECONDS` | `5`, minimum `1`; must stay above the receiver's 500 ms status heartbeat |
| `OMT_HDMI_CONNECTOR` | `auto`, `HDMI-A-1`, or `HDMI-A-2`; both supported boards have two outputs |
| `OMT_BOARD_LABEL` | Detected board, written by the installer; defaults to `Raspberry Pi` |
| `OMT_VIDEO_CEILING` | The board's decode ceiling, written by the installer; defaults to `1920x1080@60` |
| `OMT_VIDEO_CEILING_FILE` | `$OMT_CONFIG_DIR/video_ceiling.json`, the operator's override of `OMT_VIDEO_CEILING` |
| `OMT_DIAGNOSTICS_HOST_REPORT_FILE` | `/host-diagnostics/host-report.txt` |
| `OMT_DIAGNOSTICS_HOST_REQUEST_FILE` | `/host-diagnostics/request` |
| `OMT_DIAGNOSTICS_HOST_PCAP_FILE` | `/host-diagnostics/host-network.pcap` |
| `OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE` | `/host-diagnostics/host-network-pcap.txt` |
| `OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS` | `30`; must not exceed the bundle budget |
| `OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS` | `25` (host action; exported by the OpenRC watcher — container env only mirrors this into support bundles) |
| `OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS` | `60`; must stay at most the Rust Web service's bounded collection ceiling of `85` seconds |
| `OMT_DIAGNOSTICS_RECEIVE_PROBE` | `1` (enabled); accepts `1/0`, `true/false`, `yes/no`, or `on/off` |
| `OMT_DIAGNOSTICS_DOWNLOAD_LIMIT` | `10 per hour` |
| `OMT_DIAGNOSTICS_ACTION_LIMIT` | `30 per hour` |
| `OMT_RUNTIME_INTEGRITY_MANIFEST` | `/app/runtime-sha256.manifest` |
| `OMT_PROJECT_LICENSE_FILE` | `/app/legal/LICENSE` |
| `OMT_THIRD_PARTY_NOTICES_FILE` | `/app/legal/THIRD_PARTY_NOTICES.txt` |
| `OMT_REBOOT_REQUEST_FILE` | `/host-actions/reboot.request` |
| `OMT_REBOOT_RESULT_FILE` | `/host-actions/reboot.result` |
| `OMT_REBOOT_ACK_TIMEOUT_SECONDS` | `3` |
| `OMT_REBOOT_ACTION_LIMIT` | `3 per hour` |

Numeric settings reject malformed, non-finite, and out-of-range values during
application creation. Bundle budget and host timeout are cross-checked so a
misconfigured wait cannot exceed the bounded collection budget. The four rate
limits are parsed by the Rust service during startup, so a typo fails startup
instead of silently removing brute-force protection. Legacy
`OMT_DEBUG_*`,
`OMT_HOST_DEBUG_*`, and `PIPELINE_STATUS_STALE_SECONDS` variables fail startup
with migration guidance.

Boolean settings are parsed strictly as documented. An unknown value fails
startup instead of silently enabling the diagnostics receive probe.

## Persistent files

| File | Contract |
|---|---|
| `web_secret` | Mode 0600 HMAC secret. An upgrade migrates and removes the legacy `flask_secret` file. |
| `web_password` | Mode 0600 PBKDF2-SHA256 password hash. The deployer can rotate it atomically from a 12-128 byte password and restarts the service, invalidating existing sessions. Rotation is off by default; the desktop Deploy view has an explicit enable option. Legacy Werkzeug PBKDF2 and scrypt hashes remain accepted. |
| `web_sessions.json` | Schema 2 bounded HMAC-digested session registry |
| `source_target.json` | Schema 1; either `{"kind":"discovered","name":...}` or `{"kind":"direct","uri":"omt://..."}` |
| `omt/settings.xml` | `<Settings>` with at most one `<DiscoveryServer>` |
| `ssl/key.pem`, `ssl/cert.pem` | Entrypoint-managed HTTPS key/certificate |
| `receiver.log` | Appended receiver stdout/stderr; kept here so it outlives a restart |

Per-boot state is deliberately **not** in this volume. `control.lock`,
`omt.pid`, and `playback-status.json` live in `$OMT_RUNTIME_DIR`, a size-capped
tmpfs mounted at `/run/omt`. The receiver rewrites the status document on every
change and then at a 500 ms heartbeat, so holding it on the SD-card-backed
volume meant a permanent write + fsync + rename load on flash. None of it is
meaningful after a restart. An upgrade's leftover `run/` directory is removed by
the entrypoint on first boot.

A Discovery Server may be a host, `host:port`, or `omt://host:port`. Omitted
ports become 6399. mDNS remains enabled. A direct source must be an explicit
`omt://host:port` URI with no credentials, path, query, or fragment.
Saving a Discovery Server that is already effective is an idempotent operation:
the Web service preserves the existing XML and does not restart playback or
issue another durable write.

A `settings.xml` whose stored Discovery Server is not a valid one stays
correctable from Network Settings: the stored value is reported as the fault on
the page, and saving a good value replaces it. Only a document whose *structure*
cannot be trusted — a foreign root element, two `DiscoveryServer` entries, a
doctype declaration, malformed XML, or one too large to read — is refused and
left exactly as found.

## Host firewall

When an active firewall is detected, the installer permits mDNS
(`5353/udp`) and the image’s validated HTTPS port. It does not install broad
media port ranges. Site VLAN/WLAN policy must permit the OMT sender, receiver,
and optional Discovery Server traffic selected for that installation.

## HDMI

`deploy/host/install.sh --hdmi-video auto` leaves mode choice to DRM. The
installer owns a block in Alpine's active `usercfg.txt`. A forced value has
the form `HDMI-A-1:1920x1080@60`. Connector/mode state is retained in
`/etc/omt-client/installer.conf`.

The managed block sets `dtoverlay=vc4-kms-v3d`, which is correct on every
supported board — the firmware substitutes the Pi 5 variant itself. On the
pre-Pi-5 boards it also sets `gpu_mem=64`: those boards still split RAM with
the VideoCore, and under full KMS the V3D driver allocates from CMA instead, so
the split is wasted RAM. The Pi 5 has no such split and the setting is omitted
there rather than written and ignored.

Single-output boards have no `HDMI-A-2`. The installer refuses a forced mode
for it rather than writing a boot argument for a connector that never appears.

## Video limit

Each board has a decode ceiling, since the receiver decodes VMX in software and
a Cortex-A53 is not a Cortex-A76. `deploy/lib/board-profile.sh` is the table:

| Board | HDMI | Default ceiling |
|-------|------|-----------------|
| Raspberry Pi 5 | 2 | `1920x1080@60` |
| Raspberry Pi 4 Model B | 2 | `1920x1080@30,1280x720@60` |

A ceiling is a comma-separated list of `WIDTHxHEIGHT@FPS` shapes, and video is
accepted when it fits inside any one of them — which is how the Pi 4 takes
either 1080p30 or 720p60. Video above the ceiling is reported as
`unsupported-format`; it is never downscaled or frame-dropped to fit, because
the ceiling is a decode limit and resampling would not lower the decode cost.

This is separate from the attached display's mode list. Video the board can
decode but the display advertises no matching timing for is resampled into the
closest usable mode, aspect ratio preserved, rather than refused; the running
status on the dashboard names both sizes when it is.

Override it with `install.sh --max-video 1280x720@30`, or from the Web GUI's
System page. `auto` restores the board default. No ceiling may exceed
1920×1080 at 60 fps, which is what sizes the decoder's fixed allocations.
Raising a ceiling above the board default is permitted and is not validated:
a board that cannot decode the format will drop frames rather than refuse them.

Both ceilings are measured, not derived: the Pi 5 decodes the 1080p gradient
vector in 6.5 ms against a 16.7 ms budget, and the Pi 4 in 26.4 ms against
33.3 ms. Re-confirm with
`cargo test --release -p vmx-decoder --test decode_bench -- --ignored` and lower
the profile if a board cannot hold its tier.
