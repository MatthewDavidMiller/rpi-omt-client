# Codebase Reference

## Runtime

| Area | Files |
|---|---|
| Receiver CLI and command dispatch | `crates/omt-receiver/src/main.rs`, `crates/omt-receiver/src/cli.rs` |
| OMT TCP channel, subscription, and bounded frame reads | `crates/omt-receiver/src/channel.rs` |
| Source discovery (central server, then Avahi over D-Bus) | `crates/omt-receiver/src/discovery.rs`, `crates/omt-receiver/src/mdns.rs` |
| Bounded XML reads for settings and announcements | `crates/omt-receiver/src/xml.rs` |
| HDMI connector selection and hotplug checks | `crates/omt-receiver/src/connector.rs` |
| Direct KMS scanout and mode selection | `crates/omt-receiver/src/video.rs` |
| Aspect-preserving resample into a mode that is not the video's size | `crates/omt-receiver/src/scale.rs` |
| HDMI audio through ALSA | `crates/omt-receiver/src/audio.rs` |
| Playback supervisor, retry, and audio worker | `crates/omt-receiver/src/play.rs` |
| OMT wire transport and validation | `crates/omt-protocol/src/lib.rs` |
| Status projection and atomic publication | `crates/omt-receiver-core/src/lib.rs` |
| Decode-only VMX1 implementation | `crates/vmx-decoder/` |
| VMX bitstream | `crates/vmx-decoder/src/bitstream.rs` |
| Colour conversion: portable definition, AArch64 NEON BGRX kernel, and the choice between them | `crates/vmx-decoder/src/convert/scalar.rs`, `crates/vmx-decoder/src/convert/neon.rs`, `crates/vmx-decoder/src/convert/mod.rs` |
| Inverse DCT: portable definition, AArch64 NEON kernel, and the choice between them | `crates/vmx-decoder/src/idct/scalar.rs`, `crates/vmx-decoder/src/idct/neon.rs`, `crates/vmx-decoder/src/idct/mod.rs` |
| VMX conformance vectors captured from the reference decoder | `tests/vectors/vmx/` |
| First-party Rust OMT A/V test sender | `crates/omt-test-sender/`, `scripts/build-omt-test-sender.sh`, `scripts/omt-test-sender.sh` |
| Source-scoped sender firewall helper | `scripts/configure-omt-test-sender-firewall.sh` |
| Shared validation, status, and forbidden-code-point contracts | `tests/schema/` |
| Per-artifact SBOM closures from `Cargo.lock` | `scripts/cargo_lock.py` |
| OMT provenance | `third_party/omt/PROVENANCE.md`, `third_party/omt/libvmx/LICENSE.txt` |
| HTTPS composition, routes, headers, CSRF, and rate limits | `crates/omt-web/src/app.rs` |
| Credentials, legacy hash compatibility, and persistent sessions | `crates/omt-web/src/auth.rs` |
| Source discovery, playback, and status projection | `crates/omt-web/src/playback.rs` |
| Diagnostics, support archives, PCAP validation, and host actions | `crates/omt-web/src/diagnostics.rs` |
| Safe bounded/atomic I/O | `crates/omt-web/src/io.rs` |
| Persistent source and video-limit state | `crates/omt-web/src/state.rs` |
| OMT discovery-server XML | `crates/omt-web/src/network.rs` |
| Validated runtime configuration | `crates/omt-web/src/settings.rs` |
| Bounded subprocess execution | `crates/omt-web/src/command.rs` |
| Templates and design assets | `crates/omt-web/templates/`, `crates/omt-web/static/` |
| Shell process lifecycle | `deploy/container/runtime-lib.sh`, `deploy/container/start-omt.sh`, `deploy/container/control-omt.sh`, `deploy/container/entrypoint.sh` |
| Container | `deploy/Dockerfile`, `deploy/compose.yml` |
| Alpine OpenRC services | `deploy/openrc/`, `deploy/host/host-event-watcher.sh` |

The Rust Web service has one version reader used by About, Diagnostics, and the
`version.txt` support-bundle member. Build entry points prefer an explicit
`RPI_OMT_CLIENT_VERSION`, then the canonical workspace version in `Cargo.toml`,
before falling back to release metadata from Git or a versioned source directory.

`diagnostics.rs` owns both sides of the correlated host request boundary and
lays out the support ZIP. It returns the runtime check together with the exact
controller observation rendered beside it, so one page cannot contradict
itself. Raw capture is opt-in and accepted only after request-ID, size, magic,
and SHA-256 checks.

`deploy/Dockerfile` builds stripped `omt-receiver` and `omt-web` binaries. The
final appliance contains no Python interpreter, virtual environment, pip
package, or Web source tree. `runtime-sha256.manifest` covers `/app` and every
runtime binary/script under `/usr/local/bin`; the runtime SBOM contains both
Rust binary dependency closures and the final Alpine package database.

A source name's forbidden code points have one published definition, in
`tests/schema/omt-target-vectors.json`. `omt-protocol` owns the compiled table
and both Rust binaries reuse that crate, so a name the receiver would play
cannot be one the dashboard silently drops.

Playback states are pinned the same way, in
`tests/schema/playback-status-vectors.json`. Every state in it is reachable:
the receiver suite asserts `video_states` is exactly what `video_name` produces,
and Rust Web tests assert every receiver state has a public projection.

The runtime is built with checksum-locked Cargo dependencies and Rust 1.97.1
in a digest-pinned Alpine builder stage. A `scratch` stage contains only the
stripped `omt-receiver` and `omt-web`, so neither the SDK nor build toolchain is
part of the deployed image.
The ARM64 publisher fingerprints both binaries' complete local source closure
into that scratch stage: Podman's cross-stage cache can otherwise compile a
changed receiver and still reuse the old copied binary.
The builder copies only the receiver/Web crates plus the deployer
manifests and inert target stubs needed to preserve the locked workspace, so a
desktop or SSH-client source edit does not trigger another emulated ARM64
receiver compile.

Public routes are `/login`, `/logout`, `/`, `/sources/select`,
`/sources/refresh`, `/playback/restart`, `/playback/clear`,
`/settings/network`, `/settings/direct-source`, `/diagnostics`,
`/diagnostics/discovery`, `/diagnostics/runtime`, `/diagnostics/direct`,
`/diagnostics/download`, `/system`, `/system/video-limit`, `/system/reboot`, and `/about`.
All routes other than login require a current persistent session. Mutations
are POST and CSRF protected.

## Host and deployment

| Area | Files |
|---|---|
| Installer/uninstaller | `deploy/host/install.sh`, `deploy/host/uninstall.sh` |
| Factory Alpine sys-mode setup | `deploy/host/setup-sys.sh` |
| Host diagnostics | `deploy/host/host-diagnostics.sh` |
| Reboot validator | `deploy/host/host-reboot.sh`, `deploy/lib/reboot-request.sh` |
| Shared host helpers | `deploy/lib/host-validation.sh`, `deploy/lib/publication.sh`, `deploy/lib/service-install.sh` |
| HDMI boot-configuration rules | `deploy/lib/hdmi-config.sh` |
| Supported boards and decode ceilings | `deploy/lib/board-profile.sh` |
| Deployment contract | `deploy/manifest-v3.txt`, `deploy/transaction.sh` |
| CLI deployment | `scripts/deploy.sh` |
| Deployer validation, fixed actions, SSH/SFTP, deploy, Alpine sys setup, and Wi-Fi | `crates/omt-deployer-core/src/lib.rs`, `crates/omt-deployer-core/src/ssh.rs`, `crates/omt-deployer-core/src/ops.rs` |
| Workstation tooling: executable discovery, prerequisites, winget installs, ARM64 emulation, and the image-build plan | `crates/omt-deployer-core/src/tools.rs` |
| Secure command-line deployer | `crates/rpi-omt-deploy/src/main.rs` |
| Deployer CLI contract | `tests/native/test_deployer_cli.sh` |
| egui desktop deployer, its button-gating and display-scaling rules, and embedded legal texts | `crates/rpi-omt-deployer/` |
| Hash-locked Rust dependencies and supply-chain gates | `Cargo.lock`, `deny.toml`, `supply-chain/`, `scripts/check-supply-chain.sh` |
| Windows cross build | `scripts/build-windows-deployer.sh` |
| Local toolchain provisioning | `scripts/install-dev-deps.sh`, `scripts/install-hadolint.sh`, `scripts/install-trivy.sh`, `scripts/install-arm64-emulation.sh` |

`tools.rs` is where the deployer's answers about the *operator's* machine live,
as opposed to `ops.rs`, which is about the Pi. Every rule in it is a pure
function over probed values -- which `PATHEXT` suffixes to try, where Git for
Windows installs, whether a `bash.exe` is really the WSL launcher, which
program the image build should be spawned as -- because a Linux publisher is
the only host this project's gates ever run on. `Prerequisite` rows are shared
verbatim by the GUI's Setup view and the CLI's `prerequisites` subcommand, so
the two cannot describe the same workstation differently.

The native deployers default to the user's OpenSSH `known_hosts` file and can
select an alternate verified file when deployment automation keeps host keys
separately. Factory Alpine images accept `root` with an empty SSH password;
password, `none`, and keyboard-interactive methods are tried in that case.
The Alpine view (and CLI `alpine-setup`) uploads `deploy/host/setup-sys.sh`
(SFTP, or a `cat` exec fallback when a headless overlay sshd has no SFTP
subsystem) and drives hostname, IPv4 DHCP, optional Wi-Fi, user `pi`, root/`pi`
passwords, US HTTPS apk mirrors, clock sync, apk OpenSSH, and
`setup-disk -m sys` over that empty-password session. A blank SSID keeps a
boot-partition Wi-Fi association so a Wi-Fi-only factory image stays reachable.
`setup-sys.sh` releases the boot media before `setup-disk` (which otherwise
finds no available disk and exits 0 without installing), installs the
network-facing packages into the new root with `apk --root`, and verifies that
root before printing its completion marker. `tests/unit/test_setup_sys.sh`
pins those orderings.
Privileged remote operations use the provided, zeroized sudo
credential for non-root accounts and run directly for root. The SSH command
adapter continues reading through an EOF notification so that the server's
subsequent exit status remains authoritative.
On a host with no usable escalation rule, the Alpine view's root password
(or the CLI `bootstrap_root_password`) drives the fixed bootstrap through a
no-echo PTY and `su`; it is never reused for ordinary management operations.
Successful native deployments surface the installer's final summary (including
the authoritative Web URL) while omitting the noisy package transcript; every
connection secret is redacted before that summary reaches progress output.
Web-password rotation is opt-in and uses the same bounded, zeroizing stdin secret channel as
Wi-Fi management: no credential is placed in an SSH command or progress line.
The desktop Deploy view leaves the generated credential in place unless
**Rotate the Web GUI password after deploy** is enabled; Manage and the CLI
`web-password` command perform the same explicit action later.
The fixed action invokes `omt-web set-password` inside the unprivileged
container and restarts OpenRC; Rust validates the value and atomically writes
only a PBKDF2-SHA256 hash.

## Legal and release

`LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `THIRD_PARTY_SOURCE.md` are release
inputs. `scripts/check-legal-notices.py` compares shipped Rust and Alpine
dependencies to the notices. `scripts/generate-runtime-sbom.py` and
`scripts/generate-deployer-sbom.py` create CycloneDX inventories.
