# Operations

## Playback

Dashboard lists OMT names returned by the native receiver. Selecting a source
writes one atomic target record and restarts the controller. Playback states
include playing, starting, waiting for HDMI, retrying, degraded, unsupported
format, stopped, stale, and failed.

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
  OMT XML, integrity, and host diagnostic records.

The old `/debug` bookmark redirects to `/diagnostics`.

## About

`/about` displays the build version, exact project copyright, proprietary
project license, and shipped dependency notices. Legal files are bounded,
non-symlinked reads from the image.

## Reboot OS

Open `/system`, choose Reboot OS, review `/system/reboot`, and press Confirm
reboot. The POST is authenticated, CSRF protected, and rate limited. The Web
service writes a nonce/timestamp request and waits up to three seconds for the
root helper’s matching acceptance. The helper enforces a 60-second cooldown and
rejects stale, future, malformed, replayed, mis-owned, or replaced files.

If no acknowledgement arrives, do not repeatedly click reboot. Check:

```bash
sudo systemctl status omt-client-reboot.path omt-client-reboot.service
sudo journalctl -u omt-client-reboot.service
```

## Service commands

```bash
sudo systemctl status omt-client.service
sudo systemctl restart omt-client.service
docker compose -f /opt/omt-client/docker-compose.yml logs -f
```

Inside the container, `control-omt.sh status|start|stop|restart` manages the
receiver.

## Troubleshooting

- No sources: run Discovery Check; verify sender visibility, mDNS/VLAN policy,
  and the optional Discovery Server.
- Direct target fails: use the full `omt://host:port` form and run Direct Check.
- Waiting for HDMI: verify `/sys/class/drm/*/status` and the selected connector.
- Unsupported format: configure the sender for at most 1920×1080 at 60 fps.
- Video without audio: inspect ALSA devices and ELD; video remains degraded.
- Stale status: inspect controller status and `run/receiver.log`.
