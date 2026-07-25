# Architecture

The appliance is a clean OMT implementation. Shipped artifacts contain no NDI
SDK, `libndi`, NDI plugin, or GStreamer runtime.

```text
OMT network
  └─ libomtnet discovery/receive
       └─ omt-receiver (NativeAOT .NET 10)
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
       └─ root-owned systemd validator
            └─ systemctl reboot --no-block
```

## Receiver

`src/receiver/RpiOmt.Receiver` builds against audited source snapshots in
`third_party/omt`. Its dependency-free
`src/receiver/RpiOmt.Receiver.Core` owns typed CLI parsing, shared target
validation, format policy, sanitization, synchronized status projection, and
HDMI connector selection over the DRM sysfs tree.
`discover` emits bounded JSON, `probe` checks a direct OMT target, and `play`
owns receive, DRM, ALSA, hotplug, retry, and status publication. Discovered
names are NFC, have no control characters, and are at most 63 UTF-8 bytes.
Direct targets must be exact `omt://host:port` URIs.

Playback supports either Pi HDMI connector. A missing, unreadable, or
half-populated DRM tree reads as "no display connected", so the play loop
reports `waiting-for-hdmi` and retries instead of exiting. Frames over
1920×1080 or 60 fps are reported as `unsupported-format`. Interlaced input is
presented progressively without deinterlacing. Audio failure degrades playback
while video continues.

## Container and host boundary

The Alpine 3.23.5 runtime is read-only, drops all Linux capabilities, runs as
the `omt` user, and receives only DRM/ALSA devices, the OMT config volume, a
filtered Avahi D-Bus socket, diagnostics state, and the host-action directory.
The container cannot invoke systemd or write the directory containing host
action files.

Fresh diagnostics use a separate fixed-inode request channel. The Web process
writes a versioned nonce and `capture_pcap=0|1`, collects container-side data
while the root-owned oneshot runs, and accepts only a stable bounded host
report carrying that nonce. Raw capture is never started unless selected for
that download. Avahi proxy state, diagnostics, and host actions use separate
least-privilege bind mounts.

`host-reboot.sh` is installed root-owned. It accepts only a four-line
versioned reboot record from the pre-created mode-0600 request file. It checks
ownership, type, stable inode, age, future skew, replay, and cooldown; writes a
request-correlated result; then calls the fixed reboot command. No request
field can select a command or argument.

## Persistent state

The external `omt-config` volume contains credentials, sessions,
`source_target.json`, OMT `settings.xml`, TLS material, and runtime status.
Source state is one atomic schema-versioned record, not a pair of files. The
installer never migrates legacy NDI state.

## Deployment capsule

`deploy/manifest-v2.txt` defines manifest version 2: a bounded, variable-size
capsule with normalized nested paths. `deploy/transaction.sh` stages the files
under a nonce-specific directory, records the transaction's own manifest in a
durable journal, rejects symlinked ancestors, and can roll back nested paths
without trusting a later release's manifest. CLI and Windows deployment hash
every stable local snapshot, verify every remote SHA-256, recover any v1
journal with its installed v1 helper, promote the v2 set, and only then invoke
`deploy/host/install.sh`.

## Trust and legal surfaces

`LICENSE` governs project-owned code. `THIRD_PARTY_NOTICES.txt` covers shipped
runtime dependencies. The Web and Windows About pages display those texts and
their build version. The container generates a CycloneDX SBOM from final Alpine
and Python contents; the Windows publisher generates another from its locked
NuGet graph.

Pi DRM, ALSA, HDMI hotplug, and live OMT media remain hardware validation
boundaries after local unit and amd64 image checks pass.
