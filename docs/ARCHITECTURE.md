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

Connector selection tolerates a half-populated DRM tree: an unreadable
`status`, a missing `connector_id`, a zero id, or an absent `/dev/dri` node
disqualifies that card and the search continues to the next, because several
cards can expose the same connector name and the attached display may be behind
any of them.

The runtime is capped at 256 MiB and 64 processes. At 1080p it uses three DRM
scanout buffers, bounded network frames, and a persistent pool of VMX workers
with 512 KiB stacks (created once per decoder, not per frame). A 512 MiB Pi
Zero 2 W is the memory-design floor, but remains outside the supported hardware
matrix because the installer, DRM integration, and 1080p60 performance target
are Pi 5-specific.

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

The presenter decodes the next frame *before* it waits for the outstanding page
flip. DRM allows one flip per CRTC, so with three surfaces the buffer being
decoded into is neither the one on screen nor the one queued, and the decode
overlaps the previous frame's scanout; waiting first left the decoder idle for
most of every frame interval and made the third buffer pointless. A format the
display has no mode for is remembered with the reason it was refused, so an
unsupported stream stops re-reading the connector's mode list at its own frame
rate. Dumb buffers and framebuffers are kernel objects that a dropped handle
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

The shipped image contains no C/C++ toolchain or C++ standard library. Alpine's optional compiled
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
`rpi-omt-deployer` presents responsive egui Connection, Deploy, Manage, Wi-Fi,
Activity, and About views. Both reuse validators and typed management actions
from `omt-deployer-core`; secrets are zeroized and never accepted through
arguments or environment variables. That covers every buffer a secret passes
through, not only the ones it is stored in: the sudo stdin a deployment holds
for its whole run, the Wi-Fi passphrase handed to the worker thread, and the
raw `--secrets-stdin` document are all wiped rather than freed intact.

## Trust and legal surfaces

`LICENSE` governs project-owned code. `THIRD_PARTY_NOTICES.txt` covers shipped
runtime dependencies. The Web and Rust deployer About pages display those
texts and their build version; the deployer compiles them into its executable
with `include_str!` rather than reading files beside it, so a
relocated binary still states its terms. The container and deployer publishers generate
CycloneDX inventories from `Cargo.lock` and the Python lock.

The host is Alpine Linux 3.23 aarch64 in persistent sys mode. The installer
rejects other distributions, other Pi generations, and RAM-backed diskless
roots. OpenRC supervises the filtered Avahi proxy and two inotify watchers;
the Docker workload remains detached with its own restart policy.

Pi DRM, ALSA, HDMI hotplug, OpenRC boot ordering, nftables, and live OMT media
remain hardware validation boundaries after local unit and amd64 image checks
pass. QEMU has no Raspberry Pi 5 model, so the retired Raspberry Pi OS/raspi3
VM tier could not validate the supported platform and has been removed.
