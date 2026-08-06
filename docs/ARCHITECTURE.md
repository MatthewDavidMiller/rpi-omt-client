# Architecture

The appliance is a clean, bounded native OMT implementation built around its
C17 wire transport and the audited `libvmx` decoder.

```text
OMT network
  └─ C17 discovery/receive transport
       └─ omt-receiver (C17)
            ├─ dependency-free parsing/status core
            ├─ VMX decoder (libvmx)
            ├─ DRM/KMS HDMI presenter
            ├─ ALSA audio worker
            └─ atomic playback status

HTTPS operator
  └─ Gunicorn / Flask
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

`src/native/receiver` builds against the audited C17 VMX port in
`third_party/omt`. The C transport, C VMX decoder, and C receiver own
typed CLI parsing, shared target
validation, format policy, sanitization, synchronized status projection, and
HDMI connector selection over the DRM sysfs tree.
`discover` emits bounded JSON, `probe` checks a direct OMT target, and `play`
owns receive, DRM, ALSA, hotplug, retry, and status publication. Discovered
names are NFC, have no control characters, and are at most 63 UTF-8 bytes.
Direct targets must be exact `omt://host:port` URIs. The validation path uses
no locale-dependent or unbounded parsing; the native receiver conservatively
rejects decomposed combining-mark source names to preserve the Web layer's NFC
contract without carrying a large Unicode runtime.

The runtime is capped at 256 MiB and 64 processes. At 1080p it uses three DRM
scanout buffers, bounded network frames, and two VMX workers with 512 KiB
stacks. A 512 MiB Pi Zero 2 W is the memory-design floor, but remains outside
the supported hardware matrix because the installer, DRM integration, and
1080p60 performance target are Pi 5-specific.

Playback supports either Pi HDMI connector. A missing, unreadable, or
half-populated DRM tree reads as "no display connected", so the play loop
reports `waiting-for-hdmi` and retries instead of exiting. Frames over
1920×1080 or 60 fps are reported as `unsupported-format`. Interlaced input is
presented progressively without deinterlacing.

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

## Container and host boundary

The Alpine 3.23.5 runtime is read-only, drops all Linux capabilities, runs as
the `omt` user, and receives only DRM/ALSA devices, the OMT config volume, a
filtered Avahi D-Bus socket, diagnostics state, and the host-action directory.
The container cannot invoke OpenRC or write the directory containing host
action files.

Fresh diagnostics use a separate fixed-inode request channel. The Web process
writes a versioned nonce and `capture_pcap=0|1`, collects container-side data
while the root-owned oneshot runs, and accepts only a stable bounded host
report carrying that nonce. Raw capture is never started unless selected for
that download. Avahi proxy state, diagnostics, and host actions use separate
least-privilege bind mounts.

The shipped image contains no C++ standard library. Alpine's optional compiled
Python decimal accelerator is removed with its `mpdecimal`/`libstdc++` payload;
Python's standard pure-Python decimal implementation remains available and is
checked during the image build.

`host-reboot.sh` is installed root-owned. It accepts only a four-line
versioned reboot record from the pre-created mode-0600 request file. It checks
ownership, type, stable inode, age, future skew, replay, and cooldown; writes a
request-correlated result; then calls the fixed reboot command. No request
field can select a command or argument.

## Persistent state

The external `omt-config-v3` volume contains credentials, sessions,
`source_target.json`, OMT `settings.xml`, TLS material, and the receiver log.
Source state is one atomic schema-versioned record, not a pair of files. The
installer never migrates state from any other installation.

Per-boot state is kept off that volume. The control lock, PID record, and
published playback status live in `$OMT_RUNTIME_DIR`, a size-capped tmpfs
mounted at `/run/omt`; the entrypoint owns a 0700 directory inside that
world-writable mount point. The receiver republishes status on every change and
then at a 500 ms heartbeat, including while discovery, HDMI, media, or retry
waits are in progress. Status publication uses a private, uniquely named stage
and an atomic replacement. The Web consumer accepts fresh status only when its
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

The same C17 deployer sources build for the operator's own machine and for
Windows x86-64. A Linux workstation publishes both: `scripts/check-deployer.sh
--publish` stages the host package, and `scripts/build-windows-deployer.sh`
cross-compiles the Windows package with mingw-w64 through
`cmake/toolchains/windows-x86_64-mingw.cmake`. Both link SDL3, Nuklear, and
libssh2 statically from the same hash-locked archives, so the two packages
never diverge in dependency version. The cross build cannot execute what it
produces, so `scripts/verify-windows-deployer.sh` reads the shipping contract
out of the PE headers instead. Neither application is part of the appliance
image; the capsule they upload is built by the hermetic Dockerfile.

The dependency preparation step removes unused C++ examples and platform
backends before adding the dependencies to the build. Windows GameInput is not
used by the deployer and is replaced with a C no-support shim, keeping Linux
and Windows builds C-only.

`src/native/deployer/ui_main.c` lays the interface out in design units and
draws it in device pixels. Every metric is multiplied by the window's SDL
display scale, which already folds together the monitor's content scale and the
window's pixel density, and the text is rebaked from a system face at the exact
pixel height that scale asks for whenever it changes. Two breakpoints, measured
in design units rather than pixels, decide whether the form and the activity log
sit side by side and whether field captions sit beside or above their inputs, so
the same window is usable from the 620x520 minimum through a 4K desktop at 200%
scaling. The `omt_connection_validate`, `omt_options_validate`, and
`omt_wifi_validate` entry points in `src/native/deployer/core.c` are the only
source of truth for whether a tab's actions can run: the UI re-runs them a few
times a second and disables the buttons they gate rather than restating any rule.

## Trust and legal surfaces

`LICENSE` governs project-owned code. `THIRD_PARTY_NOTICES.txt` covers shipped
runtime dependencies. The Web and native deployer About pages display those
texts and their build version; the deployer compiles them into its executable
through `cmake/EmbedText.cmake` rather than reading files beside it, so a
relocated binary still states its terms. The container and deployer publishers generate
CycloneDX inventories from their native dependency locks.

The host is Alpine Linux 3.23 aarch64 in persistent sys mode. The installer
rejects other distributions, other Pi generations, and RAM-backed diskless
roots. OpenRC supervises the filtered Avahi proxy and two inotify watchers;
the Docker workload remains detached with its own restart policy.

Pi DRM, ALSA, HDMI hotplug, OpenRC boot ordering, nftables, and live OMT media
remain hardware validation boundaries after local unit and amd64 image checks
pass. QEMU has no Raspberry Pi 5 model, so the retired Raspberry Pi OS/raspi3
VM tier could not validate the supported platform and has been removed.
