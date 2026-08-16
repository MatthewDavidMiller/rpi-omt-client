# Architecture

The appliance is a clean, bounded Rust OMT implementation built around a Rust
2024 workspace and a decode-only VMX1 port.

```text
OMT network
  └─ Rust discovery/receive transport
       └─ omt-receiver (Rust)
            ├─ dependency-free parsing/status core
            ├─ VMX decoder (libvmx)
            ├─ DRM/KMS HDMI presenter
            ├─ ALSA audio worker
            └─ atomic playback status

HTTPS operator
  └─ omt-web (Rust, Axum + rustls)
       ├─ persistent authenticated sessions
       ├─ source/network/diagnostic services
       ├─ About
       └─ System reboot request

unprivileged container
  └─ fixed request file
       └─ root-owned OpenRC/inotify validator
            └─ /sbin/reboot
```

## Receiver

`crates/omt-receiver` composes Linux adapters around the audited, decode-only
`vmx-decoder` crate. `omt-protocol` owns wire transport and shared target
validation; `omt-receiver-core` owns format policy, sanitization, and
synchronized status projection; `omt-receiver` owns typed CLI parsing and HDMI
connector selection over the DRM sysfs tree.
`discover` emits bounded JSON, `probe` checks a direct OMT target, and `play`
owns receive, DRM, ALSA, hotplug, retry, and status publication. Discovered
names are NFC, have no control characters, and are at most 63 UTF-8 bytes.
Direct targets must be exact `omt://host:port` URIs. The validation path uses
no locale-dependent or unbounded parsing and enforces the Web layer's NFC
contract with Unicode normalization. Each announcement's name, withdrawal flag,
address, and port come off a single bounded XML pass rather than one reader per
field, and every rejection -- a duplicate of any requested tag, a document type
declaration, an unknown entity -- still refuses the whole document, so no field
can be read out of one another field's reader would have discarded.

The mDNS path deserializes Avahi's signals against their full signatures.
`ItemNew` and `ItemRemove` carry six fields and `ServiceResolver.Found` carries
eleven; zbus matches a body against a whole tuple's signature rather than a
prefix of it, so a tuple that stopped one field early did not read part of the
announcement, it failed outright and the browse ignored every service it was
started for. The type aliases in `mdns.rs` are named after those interfaces so
the arity has one place to be wrong. A browse that finds nothing and a browse
that cannot parse anything are otherwise indistinguishable from the outside:
both are an empty list, which is why this survived alongside a working
transport.

`endpoint_from_parts` then refuses an address no connect can use, rather than
carrying it forward as a source that only fails once selected. Direct targets
use the same rule: IPv6 link-local, unspecified, multicast, and IPv4 broadcast
literals are rejected by `parse_direct_target`, so the dashboard cannot save a
URI that only fails at connect. A resolver answers once per address family, so a service on a dual-stack link produces an
unscoped `fe80::` record beside its usable one, and the endpoint map keeps
whichever arrived last. Reaching a link-local IPv6 address needs a zone index
that neither the OMT target grammar nor the operator's direct-target box can
express -- `Endpoint::resolve` already drops one from a host string -- so the
source appeared on the dashboard and then timed out. IPv4 link-local is
deliberately still accepted: a peer in that range is reached over the one link
it is on, with no extra addressing.

Connector selection tolerates a half-populated DRM tree: an unreadable
`status`, a missing `connector_id`, a zero id, or an absent `/dev/dri` node
disqualifies that card and the search continues to the next, because several
cards can expose the same connector name and the attached display may be behind
any of them.

The runtime is capped at 128 MiB and 64 processes on every board. At 1080p it
uses three DRM scanout buffers, bounded network frames, and a persistent pool of
VMX workers with 128 KiB stacks (created once per decoder, not per frame).
Each worker owns one slice of YUV scratch rather than every slice of the frame,
and bitstream readers grow to the loaded payload instead of memset-ing the
uncompressed worst case on every frame. Worker jobs travel inline through bounded channels, and the receiver reserves
only the additional bytes needed when a network payload grows, so neither jobs
nor a second full-size payload buffer are allocated in the steady-state frame
path. The HTTPS operator uses a current-thread Tokio runtime. The 1 GiB Pi 4
Model B is the memory-design floor and is inside the supported matrix at that
cap.

Memory is not what separates the boards; decode throughput is. VMX is decoded in
software on three of the four cores every supported board has, and a 1.5 GHz
Cortex-A72 is not a 2.4 GHz Cortex-A76. So each board carries a decode ceiling
from `deploy/lib/board-profile.sh`, which the installer resolves once and passes
to the receiver as `--video-ceiling`. A ceiling is a list of shapes and a frame
is admitted when it fits inside any one of them, which is how a Pi 4 takes
either 1080p30 or 720p60 without a pixel-rate budget nobody could explain to an
operator. The ceiling is policy layered above `omt-protocol`'s absolute
1920x1080@60 limit, which still bounds every allocation, so no ceiling and no
operator override can change what the decoder is sized for.

The Pi 5 and Pi 4 ceilings are now measured rather than reasoned.
`crates/vmx-decoder/tests/decode_bench.rs` run on the hardware puts the
three-worker pool -- the row that decides a tier -- at 6.5 ms per 1080p
gradient frame on the Pi 5 against a 16.7 ms budget, and 26.4 ms on the Pi 4
against a 33.3 ms one. Both hold, the Pi 4 with the thinner margin, and a Pi 4
pointed at a 1080p60 sender refuses it with a message naming its own limit.
No supported board ships a ceiling that is only reasoned about.

The colour conversion has an AArch64 kernel for the same reason the inverse DCT
does. Once the entropy decode is spread over the pool, packing 1080p into the
8 MiB of BGRX a frame of scanout needs is what is left, and the portable kernel
spends four one-byte stores per pixel doing it. `vst4q_u8` interleaves the four
channels in the store unit instead, sixteen pixels at a time. On the Pi 5 that
takes the conversion-dominated flat vector from 5.7 ms to 4.0 ms per frame; on
the Pi 4, whose memory system rather than its arithmetic is the limit here, it
is worth about five percent on the gradient vector and nothing on the flat one.
Both kernels are checked against each other lane for lane, and the committed
conformance vectors still decode bit-exactly against the reference decoder on
both boards.

Playback supports either HDMI connector: both supported boards have two.
`HDMI-A-2` on a board that does not populate it simply never resolves and
already reads as no display. A missing, unreadable, or half-populated DRM tree
reads the same way, so the play loop reports `waiting-for-hdmi` and retries
instead of exiting. Frames above the board's ceiling are reported as
`unsupported-format`. Interlaced input is presented progressively without
deinterlacing.

The board's ceiling and the display's mode list are separate limits and are
answered separately. The ceiling says what this SoC can decode, and video above
it is refused. The mode list only says what timings the sink advertises, and a
sender's format is often not among them: HDMI sinks advertise what they were
built to show, and a set that stops at 720p is common on small panels and on
TVs whose larger timings the kernel prunes from an otherwise valid EDID.
Selection therefore prefers a mode at the video's own size — the decoder writes
that scanout buffer directly — and otherwise takes the largest usable mode the
frame reduces into, or the smallest one it has to be enlarged into, and
resamples each frame into it with the aspect ratio preserved and black bars
around it. Interlaced modes, modes whose timings give no refresh rate, and
modes outside the fixed 1920x1080 envelope are never selected. Only a display
offering no usable mode at all is now reported as `unsupported-format`, and the
running status names both sizes so a resample is visible rather than silent.

The resample is nearest-neighbour with pixel-centre sampling, and it is the
filter the budget allows: the Pi 4 tier already spends 26.4 ms of its 33.3 ms
interval decoding a 1080p frame, so a bilinear pass over the destination would
not fit. It costs one intermediate frame of ordinary memory, at most 8 MiB
against the 128 MiB container, and only for a session that needs it. **The
resampled path has not been exercised on hardware**; it is on the DRM boundary
described under trust and legal surfaces and must be validated on a Pi with a
display whose mode list does not carry the sender's format before release.

HDMI audio is resolved rather than assumed. The Pi 4 and Pi 5 register one ALSA
card per output, `vc4hdmi0` and `vc4hdmi1`, while a single-output board
registers one unindexed `vc4hdmi`. The receiver reads
`/sys/class/sound/card*/id` and takes the indexed card when it exists, a lone
`vc4hdmi` otherwise. Deriving the name from the connector alone is what made
HDMI audio fail silently on the single-output boards while video kept playing.

The PCM's software timing is set explicitly rather than left at ALSA's
defaults. `snd_pcm_hw_params` leaves `start_threshold` at a single frame, so
the device begins playing on the first write with nothing queued behind it and
the next late audio frame is an underrun the operator hears. The receiver sets
a 100 ms start threshold against a 240 ms ring, so playback only begins once
there is a cushion, and because underrun recovery re-prepares the device that
cushion is rebuilt after every underrun instead of restarting empty into the
next one. The ring is capacity, not latency: the appliance's link is Wi-Fi
carrying this session's own video, which delivers audio in bursts, and a ring
only as deep as a couple of bursts runs dry between them. The 100 ms threshold
is what sets the audio latency, chosen inside the ITU-R BT.1359 lip-sync
tolerance and partly offset by the video path's own decode and scanout delay.
Buffer and period sizes are read back from the device after `hw_params` rather
than derived from the times requested, because the device refines both and the
start threshold has to be expressed in the frames it actually chose.

Recovered underruns are counted for the life of the audio session and named in
the running status. An underrun is a gap in the sound, so reporting the count
is what separates a starved sink from a link dropping the audio stream
outright — the two are indistinguishable in the room.

The PCM is opened non-blocking, so a momentarily full ring buffer answers a
write with `EAGAIN`. That is back-pressure and it happens in steady playback
whenever the writer runs ahead of the sink; `snd_pcm_recover` handles underrun
and suspend and hands `EAGAIN` straight back, so routing it there reported a
working HDMI sink as lost and dropped playback to `degraded` on both the Pi 4
and the Pi 5. It now waits for room on the same bounded budget a device that
accepts nothing gets, which still ends the audio session for a sink that has
genuinely vanished.

The receive slice bounds how long a loop waits for a frame to *begin*, not how
long a frame may take. Those were once the same bound, and a 1080p VMX payload
is around 200 KiB — tens of milliseconds of wire time on a link already
carrying this session's own video. A frame whose header landed near the end of
a slice therefore ran out of budget mid-payload; because a half-read frame
cannot be resumed, the channel closed, the session ended, and video, audio, and
the DRM output all restarted behind the retry backoff. Nothing had stalled. The
budget was being measured from the wrong instant, and the appliance reported it
as `OMT frame was truncated by a timeout` several times a minute on 2.4 GHz.
The first byte consumed now switches the read onto its own frame budget, which
still ends a sender that genuinely stops talking mid-frame.

The audio worker reads on the same slice as the video loop. Its frames are a
fraction of a video frame's size, but a read that stalls after its first byte
cannot be resumed and ends the session, and on a link already carrying this
session's own video the smaller frame is no less likely to stall. The fifth of
the budget it used to get was not a smaller need; it was a fivefold better
chance of tearing audio down, which is what a two-board test over one Wi-Fi
radio produced repeatedly while video kept playing. The cost of the longer wait
is a recoverable ALSA underrun.

The direct probe divides its caller's bounded deadline between video and audio
instead of imposing a fixed 100 ms read slice. A compressed 1080p VMX frame is
small enough for that slice on loopback but not necessarily over a busy Pi
Wi-Fi link; consuming part of it and then timing out closes the video channel,
which previously made a healthy A/V sender report as audio-only.

The presenter returns a typed `PresentOutcome`, so the play loop distinguishes a
stream this display cannot show — which keeps the session and reports
`unsupported-format` — from a failure of the output itself, which ends the
session and retries. The two are not inferred from the error text: a genuine
`drmModeSetCrtc` failure also mentions "mode", and treating it as recoverable
reconfigured the display, tearing down and reallocating three 1080p scanout
buffers, once per decoded frame. During playback, connector
hotplug state is sampled at a bounded 500 ms cadence instead of reopening two
sysfs attributes for every decoded frame. Unchanged video/audio events reuse
their status projection rather than allocating per frame; publication still
occurs immediately on change and at the 500 ms heartbeat. Audio failure
degrades playback while video continues.

The presenter decodes the next frame *before* it waits for the outstanding page
flip. DRM allows one flip per CRTC, so with three surfaces the buffer being
decoded into is neither the one on screen nor the one queued, and the decode
overlaps the previous frame's scanout; waiting first left the decoder idle for
most of every frame interval and made the third buffer pointless. A format the
display can carry no mode for is remembered with the reason it was refused, so
an unsupported stream stops re-reading the connector's mode list at its own
frame rate. Dumb buffers and framebuffers are kernel objects that a dropped handle
does not release, so one path retires any pending flip and destroys both, for
reconfiguration and for shutdown alike. The card is opened non-blocking,
because DRM events arrive by reading it: on a blocking descriptor the wait for
a flip that a vanished display will never complete never returns, and the
500 ms flip timeout that ends such a session could not fire. All four are
review- and unit-tested only: they are on the Pi 5 DRM hardware boundary
described under trust and legal surfaces, and must be validated on hardware
before release.

## Container and host boundary

The Alpine 3.23.5 runtime is read-only, drops all Linux capabilities, runs as
the `omt` user, retains Docker's default sensitive `/proc` and `/sys` path
confinement, and receives only DRM/ALSA devices, the OMT config volume, a
filtered Avahi D-Bus socket, diagnostics state, and the host-action directory.
The container cannot invoke OpenRC or write the directory containing host
action files. A minimal injected init reaps failed receiver descendants; PID,
memory, swap, shared-memory, file-descriptor, core-dump, and temporary-filesystem
limits bound the remaining process and storage surfaces.

The appliance OpenRC service waits up to 90 seconds for Docker's API
before invoking Compose. Alpine's supervised Docker service can report itself
started while dockerd is still initializing containerd; a 30-second wait lost
that race on a Pi 4 cold start and left the appliance stopped until an
operator restarted it.

The filtered Avahi proxy runs under the container's own UID, not as `nobody`.
`xdg-dbus-proxy` serves its socket through GDBus, whose default authorization
accepts an authenticated peer only when that peer's credentials name the
server's own user, so the group grant that reaches the socket is not what
decides whether a connection survives the D-Bus handshake. Running the proxy as
anyone else drops every connection from the receiver before its first method
call, and because discovery is best-effort by design the failure surfaced as an
empty source list returned instantly rather than as an error. The installer
already resolves the image-owned UID for the state files and now publishes it
to the proxy service as well.

Fresh diagnostics use a separate fixed-inode request channel. The Web process
writes a versioned nonce and `capture_pcap=0|1`, collects container-side data
while the root-owned oneshot runs, and accepts only a stable bounded host
report carrying that nonce. Raw capture is never started unless selected for
that download. Avahi proxy state, diagnostics, and host actions use separate
least-privilege bind mounts.

The shipped image contains no Python runtime, C/C++ toolchain, or C++ standard
library. The Rust receiver and Web service are stripped release binaries.

`host-reboot.sh` is installed root-owned. It accepts only a four-line
versioned reboot record from the pre-created mode-0600 request file. It checks
ownership, type, stable inode, age, future skew, replay, and cooldown; writes a
request-correlated result; then calls the fixed reboot command. No request
field can select a command or argument.

## Persistent state

The external `omt-config-v3` volume contains credentials, sessions,
`source_target.json`, OMT `settings.xml`, TLS material, and the receiver log.
The deployer can rotate the Web credential through a fixed privileged action
when the operator enables it. The
new value travels over SSH stdin, is hashed and atomically replaced inside the
unprivileged container, and the service is restarted. Sessions bind to the
password-file digest, so sessions created under the old credential become
invalid. Deploy leaves the existing credential in place unless that option is
selected.
Source state is one atomic schema-versioned record, not a pair of files. The
installer never migrates state from any other installation.

Per-boot state is kept off that volume. The control lock, PID record, and
published playback status live in `$OMT_RUNTIME_DIR`, a size-capped tmpfs
mounted at `/run/omt`; the entrypoint owns a 0700 directory inside that
world-writable mount point. The receiver republishes status on every change and
then at a 500 ms heartbeat, including while discovery, HDMI, media, or retry
waits are in progress. Status publication uses a private, uniquely named stage
and an atomic replacement. The status directory is created once per process
rather than re-asserted on every publish: the entrypoint owns `/run/omt` and
creates it before the receiver starts, so a `create_dir_all` twice a second
forever bought nothing, and a directory that does vanish now fails the publish
audibly instead of being silently recreated. The Web consumer accepts fresh status only when its
target matches the current atomic source record, so a source change cannot
briefly present the previous receiver session as current. Keeping status on
the volume put a permanent write + fsync + rename load on SD-card-backed flash
for state that is meaningless after a restart.

## Deployment capsule

`deploy/manifest-v3.txt` defines manifest version 3: a bounded, variable-size
capsule with normalized nested paths. `deploy/transaction.sh` stages the files
under a nonce-specific directory, records the transaction's own manifest in a
durable journal, rejects symlinked ancestors, and can roll back nested paths
without trusting a later release's manifest. CLI and native GUI deployment hash
every stable local snapshot, verify every remote SHA-256, recover predecessor
journals with their installed helpers, promote the v3 set, and only then invoke
`deploy/host/install.sh`.

## Operator deployment applications

The same Rust deployer workspace builds for the operator's own machine and for
Windows x86-64. A Linux workstation publishes both: `scripts/check-deployer.sh
--publish` stages the host package, and `scripts/build-windows-deployer.sh`
cross-compiles the Windows packages. Both consume the same `Cargo.lock`;
registry packages are checksum locked and Git dependencies are denied.
`rpi-omt-deploy` provides human and JSON-lines CLI surfaces, while
`rpi-omt-deployer` presents responsive egui Setup, Connection, Alpine, Deploy, Manage,
Wi-Fi, Activity, and About views. Both reuse validators and typed management actions
from `omt-deployer-core`; secrets are zeroized and never accepted through
arguments or environment variables. That covers every buffer a secret passes
through, not only the ones it is stored in: the sudo stdin a deployment holds
for its whole run, the Wi-Fi passphrase handed to the worker thread, and the
raw `--secrets-stdin` document are all wiped rather than freed intact.
The Alpine view uploads `deploy/host/setup-sys.sh` over SFTP or a `cat` exec
fallback (factory headless sshd often has no SFTP), sets hostname, IPv4 DHCP,
optional or already-associated Wi-Fi, user `pi`, root/`pi` passwords, US HTTPS
apk mirrors after an NTP clock step, apk OpenSSH, and persistent `sys` mode.
Three orderings in that script are load-bearing. The passwords are set before
apk OpenSSH can replace the sshd carrying the session, because a stock sshd
config refuses the factory image's empty root password. The boot media is
released first with `copy-modloop`, because alpine-conf skips any disk with a
mounted partition and `setup-disk` then *exits 0* having installed nothing.
And the packages the appliance needs to rejoin the network — OpenSSH, and on
a Wi-Fi board `wpa_supplicant` with `linux-firmware-brcm` and
`wireless-regdb` — are installed into the new root with `apk --root` after the
install, because the factory image runs from a read-only modloop where the
firmware belongs to no package and `/lib/firmware` cannot be written at all.
The completion marker the deployer keys on is printed only after the new root
has been mounted and checked, so a `setup-disk` that did nothing cannot report
success.

`linux-firmware-brcm` carries the Pi 5's radio: the CYW43455 is 802.11ac 1x1
dual-band, and the package ships its `brcmfmac43455-sdio.raspberrypi,5-model-b`
firmware, NVRAM, and CLM blob. `wireless-regdb` is the other half and was
easier to miss, because leaving it out produces no error anywhere. The kernel
resolves its regulatory domain by reading `/lib/firmware/regulatory.db`; with
no database the world domain applies, and that domain contains no channels
149-165 at all. A dual-band board then associates happily on 2.4 GHz and
delivers a fraction of the throughput a 1080p OMT stream needs. Both scripts
install the database, and the firmware copy-from-running-image fallback covers
it for the same reason it covers the Broadcom firmware: `apk` has been observed
to report success while placing nothing, and on a Wi-Fi-only board that is a
trip to the SD card reader.

Every path that can write a Wi-Fi configuration defaults the country to `US`,
and every one of them preserves a country that is already declared — that is
the operator saying where the appliance is, and neither a re-deploy nor a
Wi-Fi change may relabel a radio. There are three such paths and they are
separate code because they run in different worlds:
`host_wpa_supplicant_config` for the installer, `install_wpa_config_from` in
`setup-sys.sh` for the hand-written boot-partition file (busybox `ash` on a
factory image, with no access to `deploy/lib`), and the deployer's `wpa_cli`
script for later Wi-Fi management. The last one sets the country *before* it
scans: in the world domain the upper band does not exist, so scanning first
would offer the operator 2.4 GHz networks only and hide the 5 GHz access point
they are standing next to.
Fixed management actions cross the same privilege boundary as deployment and
Wi-Fi: a non-root SSH account uses its bounded sudo-password channel, while a
root session runs the fixed command directly. Neither account needs membership
in the Docker group. Status, logs, service restart, and a deferred OS reboot
are typed actions; the GUI requires a second confirmation before the reboot.
Host-key verification defaults to OpenSSH's
`~/.ssh/known_hosts`; the CLI and GUI can select another verified file without
relaxing strict checking.

An untouched Alpine host has neither sudo nor an active doas rule. When the
Alpine root password is supplied (the GUI Alpine view, or `bootstrap_root_password`
on the CLI), the native deployers bootstrap through `su` on a bounded SSH PTY. Terminal echo is disabled before the secret
is sent; only the fixed staged bootstrap is run, and subsequent deployment
returns to the administrator's sudo credential. The root secret is zeroized
with the other authentication buffers. Remote commands retain a one-minute
idle timeout but allow the package installer up to thirty minutes while it is
still producing progress; the previous two-minute total ceiling could abort a
healthy first install on a Pi. `install.sh` stops a live appliance, then runs
`apk update` and `apk upgrade --available` with a progress meter and a
twenty-second heartbeat so a kernel or firmware fetch cannot look idle to that
timeout. Host apk fetches use a pinned allowlist of reputable US HTTPS mirrors
(kernel.org, UC Berkeley OCF, and Princeton).
The explicit root credential takes priority over an ambiguous `doas` probe:
stock Alpine can describe its inert rule set as authorization-capable and then
refuse the actual non-PTY command.

Deployment builds the appliance image on the operator's own machine, so what
that machine provides is part of the deployment contract rather than an
assumption. `omt-deployer-core`'s `tools` module owns it: executable discovery
that follows `PATHEXT`, the Windows shell locations Git for Windows installs
into, the winget packages that supply a missing prerequisite, and the plan for
invoking the image build. The Setup view and the CLI's `prerequisites`
subcommand are two renderings of the same probe.

Windows reaches `scripts/build-arm64.sh` through that shell directly rather
than through GNU Make. The Makefile recipe is a call to the script, so make
without a POSIX shell hands it to `cmd.exe`, and make with one adds nothing --
which leaves Git for Windows and Docker Desktop as the only two prerequisites
an operator has to install. Resolving the build program before spawning it is
what replaced a bare `ErrorKind::NotFound`, whose text named neither the tool
nor the remedy. Nothing in that path is observable from a Linux publisher, so
every Windows rule is a pure function tested with the Windows answer supplied,
and the behaviour of the resulting `.exe` on a real desktop remains a
validation boundary.

How the deployer's window answers a display is a set of rules, not a set of
widgets, so they live outside its view alongside the button-gating rules:
window fit against the monitor, when that fit is still the opening rather
than a drag across displays, the readable column width, when labels pair
with their fields, and the zoom bounds. Each is only observable on hardware --
a 200%-scaled laptop, a 4K desktop, a window dragged to its minimum -- so
keeping the arithmetic out of egui is what lets `cargo test` cover it without
one. The opening fit retries until the window is observed to fit, and gives
up on wall time rather than a frame count, because a `request_repaint` loop
can burn frames faster than the compositor applies `InnerSize`. The zoom
bounds are applied to the keyboard shortcuts as well as the
buttons, which is why egui's own handler is turned off: two clamps for one
control is the mistake the gating rules exist to prevent.

The native window is not centred by eframe. Both `NativeOptions.centered` and
`ViewportCommand::center_on_screen` take the primary monitor's size and use
half of it as an absolute desktop position, never adding that monitor's
origin, which on a mixed-DPI Windows desk opens the window on the wrong
display. The position is left unset: Windows then uses `CW_USEDEFAULT` (the
cursor's display, at that display's scale) and the Linux or macOS window
manager places a new window on the active display. `fit_window` then shrinks
to `current_monitor` in the window's own points.

Windows sets per-monitor DPI awareness v2 from winit at process start, so the
cross-built `.exe` needs no side-by-side manifest and
`scripts/verify-windows-deployer.sh` gains no assertion for it -- that gate
reads PE headers and can never observe DPI behaviour. The Linux publisher
cannot run a Windows or macOS window, so behaviour on those platforms is
reasoned from the pinned upstream sources rather than observed.

## Trust and legal surfaces

`LICENSE` governs project-owned code. `THIRD_PARTY_NOTICES.txt` covers shipped
runtime dependencies. The Web and Rust deployer About pages display those
texts and their build version; the deployer compiles them into its executable
with `include_str!` rather than reading files beside it, so a
relocated binary still states its terms. The container publisher generates a
CycloneDX inventory from `Cargo.lock` and the appliance's installed Alpine
package database; the deployer publisher inventories its Cargo closure.

The host is Alpine Linux 3.24 aarch64 in persistent sys mode on a Raspberry Pi
5 or Pi 4 Model B. One `linux-rpi` kernel covers both. A dual-band radio is a
support criterion: the appliance is 5 GHz only, because real-world testing
showed 2.4 GHz packet loss makes OMT playback unusable, so a board that cannot
leave 2.4 GHz cannot be a host. That is what removed the Pi Zero 2 W and the
Pi 3 tier. The installer rejects other distributions, every other board, and
RAM-backed diskless roots; `deploy/lib/board-profile.sh` and
`crates/omt-deployer-core/src/ops.rs` hold the same table for the host-side and
workstation-side gates. OpenRC supervises the filtered Avahi proxy and two inotify watchers;
the Docker workload remains detached with its own restart policy.

Host hardening disables unprivileged BPF and applies BPF JIT constant blinding
for privileged callers as well. IPv4 reverse-path filtering is pinned on, ARP
replies are limited to the incoming interface, IPv6 router advertisements and
SLAAC are refused, and apk repositories are pinned to US HTTPS mirrors.
SSH keeps root key access for recovery but
limits every login to root or the administrative `wheel` group; password root
login and all forwarding paths remain disabled. Onboard Bluetooth is disabled
in firmware, blocked at runtime, and blacklisted as kernel modules. The CPU
frequency governor is pinned to `performance` so decode on the Pi 4 does not
drop below its ceiling under schedutil. Wi-Fi power save is pinned off so
brcmfmac does not drop mDNS or OMT datagrams. Time synchronization is enabled
through the host's existing ntpd, or chrony when nothing else is providing a
clock.

The native deployers reboot the Pi after a successful install, wait for SSH to
return, and wait for the appliance container. They surface the first-start Web
GUI password from the container logs when it is still retained. `make deploy`
does the same.

Pi DRM, ALSA, HDMI hotplug, OpenRC boot ordering, nftables, and live OMT media
remain hardware validation boundaries after local unit and amd64 image checks
pass, now once per supported board rather than once. Per-board decode ceilings
join that list: they are reasoned from core count and clock and are confirmed or
refuted only by running the decode bench on the hardware. QEMU models none of
these SoCs, so the retired Raspberry Pi OS/raspi3 VM tier could not validate the
supported platform and has been removed.
