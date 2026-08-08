# Operations

## Web GUI layout

Every page shares a sticky header holding the hostname, the primary navigation
(Dashboard, Network Settings, Diagnostics, System, About), and Log out. The
layout is fluid: cards reflow into one column on phone-width screens and use
the full width of large monitors, and the palette follows the browser's light
or dark preference.

## Playback

Dashboard lists OMT names returned by the native receiver. Selecting a source
writes one atomic target record and restarts the controller. Playback states
include playing, starting, waiting for discovery, waiting for HDMI, retrying,
degraded, unsupported format, stopped, stale, and configuration-error.
Diagnostics and Network Settings surface the same corrupt-target error rather
than labeling it as "not configured".

Stop & Clear first stops the managed process and only then removes the target.
Restart requires an existing target. Controller process identity includes PID,
`/proc` start time, and executable/cmdline matching.

## Network Settings

mDNS discovery is always available through the filtered Avahi proxy. Configure
one optional Discovery Server for routed networks; port 6399 is the default.
Direct playback requires `omt://host:port`.

## Diagnostics

`/diagnostics` exposes:

- `/diagnostics/discovery` — receiver JSON discovery;
- `/diagnostics/runtime` — receiver version and controller state;
- `/diagnostics/direct` — bounded direct-target probe;
- `/diagnostics/download` — version, runtime, discovery, controller, playback,
  current-target receive probe, OMT XML, integrity, and a freshly correlated
  host report. An unchecked-by-default checkbox lets the operator opt into a
  validated raw PCAP for that download only.

`/debug` is removed. Bundles are named `omt-diagnostics-<UTC>.zip`; capture
metadata is always present, while raw PCAP data is absent unless explicitly
requested and successfully validated.

## About

`/about` displays the build version, exact project copyright, MIT project
license, and shipped dependency notices. Legal files are bounded,
non-symlinked reads from the image.

## Video limit

`/system` shows the detected board and the decode limit in force. Video above
it is reported as `unsupported-format` rather than downscaled. POST
`/system/video-limit` with one or more comma-separated `WIDTHxHEIGHT@FPS`
values to override the board default, or an empty value to restore it; the
change restarts playback. Raising the limit past the board default is allowed
and is flagged on the page: a board that cannot decode the format drops frames
instead of refusing it, which on the dashboard looks like a network fault.

## Reboot OS

Open `/system`, choose Reboot OS, review `/system/reboot`, and press Confirm
reboot. The POST is authenticated, CSRF protected, and rate limited. The Web
service writes a nonce/timestamp request and waits up to three seconds for the
root helper’s matching acceptance. The helper enforces a 60-second cooldown and
rejects stale, future, malformed, replayed, mis-owned, or replaced files.

If no acknowledgement arrives, do not repeatedly click reboot. Check:

```bash
sudo rc-service omt-client-reboot status
sudo tail -n 200 /var/log/messages
```

## Service commands

```bash
sudo rc-service omt-client status
sudo rc-service omt-client restart
docker compose -f /opt/omt-client/deploy/compose.yml logs -f
```

Host security and low-memory state:

```bash
sudo rc-status --all
sudo nft list table inet omt_client
zramctl
docker inspect --format '{{.HostConfig.Memory}} {{.HostConfig.PidsLimit}}' omt-client
```

Inside the container, `control-omt.sh status|start|stop|restart` manages the
receiver.

## Troubleshooting

- No sources: run Discovery Check; verify sender visibility, mDNS/VLAN policy,
  and the optional Discovery Server.
- Direct target fails: use the full `omt://host:port` form and run Direct Check.
- Waiting for HDMI: verify `/sys/class/drm/*/status` and the selected connector.
- Unsupported format: the dashboard names this appliance's limit. Configure
  the sender within it, or raise it on `/system`. Limits are per board; see
  the video limit table in [CONFIGURATION.md](CONFIGURATION.md).
- Video without audio: inspect ALSA devices and ELD; video remains degraded.
- Service does not start after install: confirm Alpine loaded `linux-rpi`, then
  inspect `/dev/dri`, `/dev/snd`, `dmesg`, and `rc-service omt-client status`.
- Wi-Fi save fails: reboot once after installation, then verify `wlan0`,
  `rc-service wpa_supplicant status`, and the `/run/wpa_supplicant/wlan0`
  control socket. The installer enables durable configuration updates.
- Stale status: inspect controller status and `receiver.log` in the config
  volume. Per-boot state (lock, PID record, published status) lives on a tmpfs
  at `/run/omt/state` and is gone after a restart; the log is kept on the volume
  precisely so it survives one.
