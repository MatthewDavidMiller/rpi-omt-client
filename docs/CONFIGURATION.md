# Configuration

## Environment

| Variable | Default / purpose |
|---|---|
| `WEB_PORT` | `5000`, HTTPS listener |
| `RPI_OMT_CLIENT_VERSION` | Build version embedded in image/receiver |
| `RPI_OMT_CLIENT_VERSION_FILE` | `/app/RPI_OMT_CLIENT_VERSION` |
| `OMT_CONFIG_DIR` | `/etc/omt` persistent application state |
| `OMT_STORAGE_PATH` | `/etc/omt/omt`, libomtnet storage |
| `OMT_RUNTIME_CONFIG_FILE` | `$OMT_STORAGE_PATH/settings.xml` |
| `OMT_PASSWORD_FILE` | `$OMT_CONFIG_DIR/web_password` |
| `OMT_SESSION_LIFETIME_SECONDS` | `43200` |
| `OMT_MAX_REQUEST_BYTES` | `16384` request body ceiling, minimum `1024` |
| `OMT_LOGIN_RATE_LIMIT` | `5 per minute` |
| `OMT_RECEIVER_COMMAND` | `/usr/local/bin/omt-receiver` |
| `OMT_CONTROL_COMMAND` | `/usr/local/bin/control-omt.sh` |
| `OMT_CONTROL_TIMEOUT_SECONDS` | `8` |
| `OMT_SOURCE_CACHE_TTL_SECONDS` | `5` |
| `OMT_SOURCE_TARGET_FILE` | `$OMT_CONFIG_DIR/source_target.json` |
| `OMT_PLAYBACK_STATUS_FILE` | `$OMT_CONFIG_DIR/run/playback-status.json` |
| `OMT_PLAYBACK_STATUS_STALE_SECONDS` | `5` |
| `OMT_HDMI_CONNECTOR` | `auto`, `HDMI-A-1`, or `HDMI-A-2` |
| `OMT_DIAGNOSTICS_HOST_REPORT_FILE` | `/host-diagnostics/host-report.txt` |
| `OMT_DIAGNOSTICS_HOST_REQUEST_FILE` | `/host-diagnostics/request` |
| `OMT_DIAGNOSTICS_HOST_PCAP_FILE` | `/host-diagnostics/host-network.pcap` |
| `OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE` | `/host-diagnostics/host-network-pcap.txt` |
| `OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS` | `30` |
| `OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS` | `25` |
| `OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS` | `60` |
| `OMT_DIAGNOSTICS_RECEIVE_PROBE` | enabled |
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
application creation. Legacy `OMT_DEBUG_*`, `OMT_HOST_DEBUG_*`, and
`PIPELINE_STATUS_STALE_SECONDS` variables fail startup with migration guidance.

## Persistent files

| File | Contract |
|---|---|
| `flask_secret` | Mode 0600 signing/HMAC secret |
| `web_password` | Mode 0600 Werkzeug password hash |
| `web_sessions.json` | Bounded HMAC-digested session registry |
| `source_target.json` | Schema 1; either `{"kind":"discovered","name":...}` or `{"kind":"direct","uri":"omt://..."}` |
| `omt/settings.xml` | `<Settings>` with at most one `<DiscoveryServer>` |
| `ssl/key.pem`, `ssl/cert.pem` | Entrypoint-managed HTTPS key/certificate |
| `run/playback-status.json` | Atomic receiver state and timestamp |

A Discovery Server may be a host, `host:port`, or `omt://host:port`. Omitted
ports become 6399. mDNS remains enabled. A direct source must be an explicit
`omt://host:port` URI with no credentials, path, query, or fragment.

## Host firewall

When an active firewall is detected, the installer permits mDNS
(`5353/udp`) and the image’s validated HTTPS port. It does not install broad
media port ranges. Site VLAN/WLAN policy must permit the OMT sender, receiver,
and optional Discovery Server traffic selected for that installation.

## HDMI

`deploy/host/install.sh --hdmi-video auto` leaves mode choice to DRM. A forced value has
the form `HDMI-A-1:1920x1080@60`. Connector/mode state is retained in
`/etc/omt-client/installer.conf`.
