# Operations

> **Beta — not production ready.** Every `0.9.x` release is a beta. Treat an
> appliance running one as equipment under evaluation: keep physical access to
> it, expect behaviour to change between releases, and collect a diagnostics
> bundle before reporting anything. **Version 1.0 will be the first production
> release.**

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
  host report. The host report includes the effective kernel and SSH hardening
  values, so configuration drift is visible rather than inferred from the
  installer source. An unchecked-by-default checkbox lets the operator opt
  into a validated raw PCAP for that download only.

`/debug` is removed. Bundles are named `omt-diagnostics-<UTC>.zip`; capture
metadata is always present, while raw PCAP data is absent unless explicitly
requested and successfully validated.

## About

`/about` displays the build version, exact project copyright, MIT project
license, and shipped dependency notices. Legal files are bounded,
non-symlinked reads from the image.

## Video limit

`/system` shows the detected board and the decode limit in force. Video above
it is reported as `unsupported-format` rather than downscaled; resampling would
not lower the decode cost the limit exists to bound. Video the board can decode
but the display has no matching mode for is a different case and is resampled
into the closest mode rather than refused — the dashboard's running detail then
names both the sender's size and the display's mode. POST
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
sudo sh -c '. /etc/conf.d/omt-client; docker compose --env-file "$OMT_COMPOSE_ENV_FILE" -f "$OMT_COMPOSE_FILE" logs -f omt-client'
```

To retrieve the initial Web GUI password before the bounded container log
rotates, omit `-f` and filter the first-start message:

```bash
sudo sh -c '. /etc/conf.d/omt-client; docker compose --env-file "$OMT_COMPOSE_ENV_FILE" -f "$OMT_COMPOSE_FILE" logs omt-client' \
  | sed -n '/Web UI password/,+1p'
```

The plaintext is emitted only when the password is first created. The
persistent `web_password` value is a one-way hash, not a recoverable copy.

### Change the Web GUI password

Rotation is optional and off by default. In the desktop deployer, enable
**Rotate the Web GUI password after deploy** on the **Deploy** view, or open
**Manage**, enter and confirm a new password, and select **Change Web GUI
password**. The password must contain 12-128 UTF-8 bytes
and no control characters. The deployer sends it only over the authenticated
SSH stdin channel, atomically replaces the PBKDF2-SHA256 hash, and restarts the
appliance. Every existing Web session is invalid after the restart.

The CLI exposes the same operation without placing a secret in process
arguments:

```bash
printf '%s\n' \
  '{"password":"SSH_PASSWORD","sudo_password":"SUDO_PASSWORD","web_password":"NEW_WEB_PASSWORD"}' \
  | rpi-omt-deploy --host pi.example --username pi --secrets-stdin web-password
```

For root SSH, omit `sudo_password`. Prefer the desktop prompt or a protected
input source in automation; the literal JSON is only a field-layout example.
The operation refuses an active `OMT_WEB_PASSWORD` emergency override.

Host security and low-memory state:

```bash
sudo rc-status --all
sudo nft list table inet filter
zramctl
docker inspect --format '{{.HostConfig.Memory}} {{.HostConfig.PidsLimit}}' omt-client
docker inspect --format '{{json .HostConfig.MaskedPaths}} {{json .HostConfig.ReadonlyPaths}}' omt-client
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
  the video limit table in [CONFIGURATION.md](CONFIGURATION.md). A display
  that advertises no timing at the sender's size no longer reports this: the
  picture is resampled into the closest mode instead. The message now only
  appears for video over the board's limit, or for a display offering no
  usable mode at all.
- Picture is soft, or has black bars: the display's mode list carries nothing
  at the sender's size, so the frame is being resampled into the closest mode.
  The running detail on the dashboard names both sizes. Check the display's
  advertised modes with `cat /sys/class/drm/card0-HDMI-A-1/modes` on the Pi;
  if the mode you expect is in the EDID but not that list, the kernel pruned
  it, and `install.sh --hdmi-video HDMI-A-1:1920x1080@60` forces it onto the
  kernel command line.
- Video without audio: inspect ALSA devices and ELD; video remains degraded.
- Video is choppy and keeps dropping out, with the dashboard cycling through
  `retrying`: read the detail. `OMT frame was truncated by a timeout` means a
  frame started arriving and did not finish inside the receiver's budget, which
  on this appliance almost always means the link cannot carry the stream. Check
  the band first. A support bundle's **Wi-Fi bands, regulatory domain, and
  firmware** section answers that directly: a 2.4 GHz association at 72 MBit/s
  has roughly a sixth of the throughput a 1080p OMT stream needs, and every
  retry restarts video, audio, and the display output together. If the phy
  reports no channels above 5 GHz, or `regulatory.db absent`, the radio is in
  the world domain rather than genuinely single-band — see the regulatory
  entry below.
- Wi-Fi will not associate, or the deployer's Wi-Fi view offers no networks:
  the appliance is 5 GHz only. `wpa_supplicant.conf` carries a `freq_list` of
  the US 5 GHz channels and nothing else, so a 2.4 GHz SSID is never scanned
  for and cannot be joined. This is deliberate — real-world testing showed
  2.4 GHz packet loss makes OMT playback unusable — and it is why the Pi Zero
  and Pi 3 tiers are no longer supported hosts. Move the SSID to 5 GHz or use
  Ethernet.
- Associated on 2.4 GHz after an upgrade: the restriction lands in
  `wpa_supplicant.conf` and takes effect at the next association, so a board
  that was already on 2.4 GHz when it was upgraded stays there until it
  reassociates. The installer leaves that link connected on purpose — it is
  usually carrying the deployment's own SSH session — and prints a warning
  naming the frequency. Move the SSID to 5 GHz or attach Ethernet **before**
  rebooting, or the board will not come back on Wi-Fi.
- No 5 GHz channels offered at all: this is regulatory, not hardware. The
  kernel reads its domain from `/lib/firmware/regulatory.db`, which
  `wireless-regdb` installs; without it the world domain applies, and that
  domain has no channels 149-165 at all. Most US 5 GHz access points sit in
  exactly that range. Confirm with `iw reg get` (country `00` is the world
  domain) and `iw phy phy0 info`, which lists no 5 GHz frequencies when the
  database is missing. `install.sh` installs the database and writes
  `country=` into `wpa_supplicant.conf`, defaulting to `US` when nothing
  declares one; a board provisioned before that needs one more deploy. To
  place the appliance in another domain, set `country=` in
  `/etc/wpa_supplicant/wpa_supplicant.conf` and redeploy — a declared country
  is preserved, never overwritten. Both supported boards use the CYW43455
  (802.11ac 1x1), which negotiates VHT80 at 433 MBit/s on 5 GHz.
- Choppy audio, or audio dropping out in short gaps: the dashboard's playing
  detail counts the underruns. Each one is a gap, and they mean audio is
  arriving later than HDMI consumes it. Check the link first — a support
  bundle's `wlan0 link` section gives the PHY rate and signal, and 2.4 GHz
  carrying 1080p video alongside the audio stream is the usual cause. The
  bundle's ALSA playback stream state section shows the negotiated buffer,
  period, and start threshold, and whether the stream is in `XRUN`.
- Service does not start after install: confirm Alpine loaded `linux-rpi`, then
  inspect `/dev/dri`, `/dev/snd`, `dmesg`, Docker readiness, and
  `rc-service omt-client status`.
- Web password is not in the logs: the one-time first-start message has rotated
  or the appliance was initialized previously. Redeployment does not generate a
  replacement because it preserves credentials. Do not delete or edit the
  persistent hash while the service is running.
- Wi-Fi save fails: reboot once after installation, then verify the wireless
  interface (`iw dev`), `rc-service wpa_supplicant status`, and the
  `/run/wpa_supplicant/<iface>` control socket. The installer enables durable
  configuration updates and pins Wi-Fi power save off (brcmfmac otherwise
  drops mDNS and OMT datagrams).
- Stale status: inspect controller status and `receiver.log` in the config
  volume. Per-boot state (lock, PID record, published status) lives on a tmpfs
  at `/run/omt/state` and is gone after a restart; the log is kept on the volume
  precisely so it survives one.

### Locked out after installing: no SSH and no web UI, but ping still replies

A Pi that answers ICMP while refusing both `22` and the web port has a
conflicting nftables ruleset. Netfilter runs **every** base chain registered on
the `input` hook, and an `accept` only ends the chain it appears in — it does
not stop a later chain from dropping the packet. So an appliance rule that
accepts SSH in its own table is still discarded by any other table whose input
chain ends in `policy drop`, such as the one Alpine's `nftables` package ships.

Versions before this fix installed exactly that arrangement. Recovery needs the
console or the SD card, because no network path survives:

```bash
# On the Pi's console, as root:
nft flush ruleset                       # restores access immediately
rm -f /etc/nftables.d/omt-client.nft    # stops it coming back at boot
rc-update del nftables boot
```

With the card in another machine, delete `etc/nftables.d/omt-client.nft` from
the root filesystem and remove the `nftables` symlink under
`etc/runlevels/boot/`, then boot the Pi normally and re-deploy.

The installer now appends its accepts to the host's own `inet filter input`
chain instead of creating a second table, so they are evaluated in the same
chain as the `policy drop` that would otherwise override them.
`tests/unit/test_firewall_reachability.sh` proves this with real connections in
a network namespace rather than by inspecting the ruleset text.
