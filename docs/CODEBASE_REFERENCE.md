# Codebase Reference

## Runtime

| Area | Files |
|---|---|
| Receiver CLI and command dispatch | `crates/omt-receiver/src/bin/omt-receiver.rs` |
| OMT TCP channel, subscription, and bounded frame reads | `crates/omt-receiver/src/channel.rs` |
| Source discovery (central server, then Avahi over D-Bus) | `crates/omt-receiver/src/discovery.rs`, `crates/omt-receiver/src/mdns.rs` |
| First-party D-Bus wire client: SASL EXTERNAL, marshalling, bounded messages | `crates/omt-receiver/src/dbus.rs` |
| Bounded XML reads for settings and announcements | `crates/omt-common/src/xml.rs` |
| HDMI connector selection and hotplug checks | `crates/omt-receiver/src/connector.rs` |
| Direct KMS scanout and mode selection: planar YCbCr through the display hardware by atomic mode-setting, BGRX by the legacy ioctls as the fallback; raw ioctls against hand-declared uapi held to the kernel headers by the ABI probe | `crates/omt-receiver/src/video.rs`, `crates/omt-sys/src/drm.rs` |
| Placement and sampling tables for a mode that is not the video's size (the resample runs in the decoder's placed output) | `crates/omt-receiver/src/scale.rs` |
| HDMI audio through ALSA | `crates/omt-receiver/src/audio.rs`, `crates/omt-sys/src/alsa.rs` |
| Frame handoff: newest video frame, audio in arrival order | `crates/omt-receiver/src/handoff.rs` |
| Playback supervisor, retry, and audio worker | `crates/omt-receiver/src/play.rs` |
| OMT wire transport and validation | `crates/omt-protocol/src/lib.rs` |
| Video ceilings, status projection, and atomic publication | `crates/omt-receiver-core/src/ceiling.rs`, `crates/omt-receiver-core/src/status.rs` |
| Decode-only VMX1 implementation and its worker pool | `crates/omt-vmx/` |
| VMX bitstream and plane decoding | `crates/omt-vmx/src/bits.rs`, `crates/omt-vmx/src/plane.rs` |
| Entropy lookahead table and its generator | `crates/omt-vmx/src/ac_lookahead.rs`, `tools/gen/gen_vmx_tables.py` |
| Colour conversion: portable definition and AArch64 assembly BGRX kernel | `crates/omt-vmx/src/convert.rs`, `crates/omt-vmx/asm/convert_aarch64.s` |
| Inverse DCT: portable definition, x86-64 SSE2 kernel, and AArch64 assembly kernel | `crates/omt-vmx/src/idct.rs`, `crates/omt-vmx/src/idct_sse2.rs`, `crates/omt-vmx/asm/idct_aarch64.s` |
| Assembler macros the kernels are written against, assembled by `global_asm!` | `crates/omt-vmx/asm/prelude.s`, `crates/omt-vmx/src/asm.rs` |
| Decode throughput measurement for per-board ceilings | `crates/omt-vmx/src/bin/bench_vmx.rs` |
| VMX conformance vectors captured from the reference decoder | `tests/vectors/vmx/` |
| First-party OMT A/V test sender | `crates/omt-test-sender/`, `scripts/build-omt-test-sender.sh`, `scripts/omt-test-sender.sh` |
| Source-scoped sender firewall helper | `scripts/configure-omt-test-sender-firewall.sh` |
| Shared validation, status, and forbidden-code-point contracts | `tests/schema/` |
| Bounded buffers, strict JSON, NFC, file and process I/O | `crates/omt-common/` |
| The build version, stamped by this one crate's build script so a version change recompiles one crate | `crates/omt-common/build.rs` |
| NFC tables generated from the pinned Unicode data | `tools/gen/gen_nfc_tables.py`, `third_party/unicode/` |
| OMT provenance | `third_party/omt/PROVENANCE.md`, `third_party/omt/libvmx/LICENSE.txt` |
| HTTPS server: poll loop, OpenSSL TLS, request parsing, connection and time limits | `crates/omt-web/src/http.rs` |
| Routes, headers, CSRF, sessions, and rate limits | `crates/omt-web/src/app.rs` |
| Credentials, legacy hash compatibility, and persistent sessions | `crates/omt-web/src/auth.rs` |
| OpenSSL EVP adapters: hashes, HMAC, PBKDF2, scrypt | `crates/omt-crypto/src/lib.rs`, `crates/omt-sys/src/evp.rs` |
| Source discovery, playback, and status projection | `crates/omt-web/src/playback.rs` |
| Diagnostics, support archives, PCAP validation, and host actions | `crates/omt-web/src/diagnostics.rs`, `crates/omt-web/src/zip.rs` |
| Safe bounded/atomic I/O | `crates/omt-common/src/fsio.rs` |
| Persistent source and video-limit state | `crates/omt-web/src/state.rs` |
| OMT discovery-server XML | `crates/omt-web/src/network.rs` |
| Validated runtime configuration | `crates/omt-web/src/settings.rs` |
| Bounded subprocess execution | `crates/omt-common/src/proc.rs` |
| Templates (compiled to Rust by `tools/gen/gen_templates.py`) and design assets | `crates/omt-web/templates/`, `crates/omt-web/static/` |
| Shell process lifecycle | `deploy/container/runtime-lib.sh`, `deploy/container/start-omt.sh`, `deploy/container/control-omt.sh`, `deploy/container/entrypoint.sh` |
| Container | `deploy/Dockerfile`, `deploy/compose.yml` |
| Alpine OpenRC services | `deploy/openrc/`, `deploy/host/host-event-watcher.sh` |

The Web service has one version reader used by About, Diagnostics, and the
`version.txt` support-bundle member. Build entry points prefer an explicit
`RPI_OMT_CLIENT_VERSION`, then the canonical version in `VERSION`,
before falling back to release metadata from Git or a versioned source directory.
Because that version is baked into every artifact at build time, the builds that
publish one run from `.githooks/post-commit` rather than the commit gate.

`diagnostics.rs` owns both sides of the correlated host request boundary and
lays out the support ZIP. It returns the runtime check together with the exact
controller observation rendered beside it, so one page cannot contradict
itself. Raw capture is opt-in and accepted only after request-ID, size, magic,
and SHA-256 checks.

`deploy/Dockerfile` builds stripped `omt-receiver` and `omt-web` binaries. The
final appliance contains no Python interpreter, virtual environment, pip
package, or Web source tree. `runtime-sha256.manifest` covers `/app` and every
runtime binary/script under `/usr/local/bin`; the runtime SBOM lists both
first-party binaries, the Unicode data, and the final Alpine package database.

A source name's forbidden code points have one published definition, in
`tests/schema/omt-target-vectors.json`. `crates/omt-protocol` owns the compiled table
and both binaries link that crate, so a name the receiver would play
cannot be one the dashboard silently drops.

Playback states are pinned the same way, in
`tests/schema/playback-status-vectors.json`. Every state in it is reachable:
the receiver suite asserts `video_states` is exactly what `video_name` produces,
and the Web tests assert every receiver state has a public projection.

The runtime is built from the Rust workspace with the pinned Rust toolchain
against Alpine's musl, ALSA, and OpenSSL in a digest-pinned Alpine builder
stage. A `scratch` stage contains only the
stripped `omt-receiver` and `omt-web`, so neither the SDK nor build toolchain is
part of the deployed image.
The ARM64 publisher fingerprints both binaries' complete local source closure
into that scratch stage: Podman's cross-stage cache can otherwise compile a
changed receiver and still reuse the old copied binary.
That compile is a cross-compile on the build machine against an Alpine sysroot
(`scripts/make-sysroot.sh`); only the runtime stage's package installs run
under emulation.

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
| Post-install rename | `deploy/host/set-hostname.sh` |
| Host diagnostics | `deploy/host/host-diagnostics.sh` |
| Reboot validator | `deploy/host/host-reboot.sh`, `deploy/lib/reboot-request.sh` |
| Shared host helpers | `deploy/lib/host-validation.sh`, `deploy/lib/publication.sh`, `deploy/lib/service-install.sh` |
| HDMI boot-configuration rules | `deploy/lib/hdmi-config.sh` |
| Supported boards and decode ceilings | `deploy/lib/board-profile.sh` |
| Deployment contract | `deploy/manifest-v3.txt`, `deploy/transaction.sh` |
| Capsule embedded into the deployer, compiled in with `include_bytes!` by a build script that reads the manifest | `crates/omt-capsule/build.rs`, `crates/omt-deploy/src/capsule.rs` |
| CLI deployment | `scripts/deploy.sh` |
| Deployer validation, fixed actions, deploy, Alpine sys setup, rename, and Wi-Fi | `crates/omt-deploy/src/core.rs`, `crates/omt-deploy/src/ops.rs` |
| First-party SSH client: transport, key exchange (`crates/omt-deploy/src/ssh/kex.rs`), host keys and `known_hosts`, user keys and bcrypt_pbkdf, authentication, channels, SFTP | `crates/omt-deploy/src/ssh/` |
| Deployer platform layer for Linux and Windows | `crates/omt-deploy/src/sys.rs`, `crates/omt-sys/src/posix.rs`, `crates/omt-sys/src/win32.rs` |
| Local Alpine SD-card preparation, verified headless-overlay download over a bounded HTTPS client, and initial Wi-Fi configuration | `crates/omt-deploy/src/sd_card.rs`, `crates/omt-deploy/src/https.rs` |
| Workstation tooling: executable discovery, prerequisites, winget installs, ARM64 emulation, and the image-build plan | `crates/omt-deploy/src/tools.rs` |
| Secure command-line deployer | `crates/rpi-omt-deploy/src/main.rs` |
| Deployer CLI contract, and the SSH client held to OpenSSH | `tests/native/test_deployer_cli.sh`, `tests/integration/test_ssh_client.sh` |
| Browser deployer for Linux and Windows: the state model (`crates/rpi-omt-deploy-gui/src/app.rs`), the loopback server and its API (`crates/rpi-omt-deploy-gui/src/server.rs`), the folder picker (`crates/rpi-omt-deploy-gui/src/fs.rs`), the embedded legal texts, and the page (`crates/rpi-omt-deploy-gui/web/`) | `crates/rpi-omt-deploy-gui/` |
| Job definitions both deployer frontends share, so a deployment means one thing | `crates/omt-deploy/src/jobs.rs` |
| Gate toolbox image: every compiler, linter, and scanner the gates use | `tools/toolbox/Dockerfile` |
| Target sysroot for the cross-compiled appliance build and AArch64 suites | `scripts/make-sysroot.sh` |
| AArch64 suites: cross-compiled, run under qemu-user | `scripts/test-arm64.sh` |
| Rust static gates, sanitized suites and fuzzing, and the ABI probe | `scripts/check-rust.sh`, `scripts/rust-sanitized.sh`, `tests/abi/check_abi.py` |
| Every foreign declaration (libc, OpenSSL, ALSA, DRM, Win32) | `crates/omt-sys/` |
| Runs a gate inside the toolbox; the only thing the host needs is Docker or Podman | `scripts/toolbox.sh` |
| Reads the Linux deployer's static-linkage guarantee back out of the ELF headers | `scripts/verify-linux-deployer.sh` |
| Supply-chain gate and the pinned Windows OpenSSL | `scripts/check-supply-chain.sh`, `scripts/build-openssl.sh` |
| Windows cross build | `scripts/build-windows-deployer.sh` |
| Local commit gate, publishing builds, and GitHub Release publisher | `.githooks/pre-commit`, `.githooks/post-commit`, `scripts/setup-hooks.sh`, `scripts/publish-github-release.sh` |
| Local toolchain provisioning | `scripts/install-dev-deps.sh`, `scripts/install-hadolint.sh`, `scripts/install-trivy.sh`, `scripts/install-arm64-emulation.sh` |

`tools.rs` is where the deployer's answers about the *operator's* machine live,
as opposed to `ops.rs`, which is about the Pi. Every rule in it is a pure
function over probed values -- which `PATHEXT` suffixes to try, where Git for
Windows installs, whether a `bash.exe` is really the WSL launcher, which
program the image build should be spawned as -- because a Linux publisher is
the only host this project's gates ever run on. Nothing in it applies to an
operator any more: the build compiles the whole manifest-v3 capsule, image
archive included, into both executables, so a deployment reads no local file
and builds nothing. The rows describe the embedded capsule unless `--project`
names a checkout, which is the developer path `--rebuild-image` extends.

The native deployers default to the user's OpenSSH `known_hosts` file and can
select an alternate verified file when deployment automation keeps host keys
separately. Factory Alpine images accept `root` with an empty SSH password;
password, `none`, and keyboard-interactive methods are tried in that case.
Before first boot, every deployer frontend can prepare the mounted boot
partition of a freshly flashed Alpine image. The shared job downloads the
version-pinned headless bootstrap over HTTPS, bounds it to 1 MiB, verifies its
committed SHA-512, and writes it beside a Linux-text `wpa_supplicant.conf`.
That configuration stores the SSID as hex and the derived WPA PSK rather than
the operator's plaintext passphrase. The selected directory must carry the
Alpine marker plus the Raspberry Pi `config.txt` and boot subdirectory, so an
arbitrary workstation folder cannot be mistaken for the card.
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
Privileged remote operations use the provided, wiped-on-free sudo
credential for non-root accounts and run directly for root. The SSH command
adapter continues reading through an EOF notification so that the server's
subsequent exit status remains authoritative.
On a host with no usable escalation rule, the Alpine view's root password
(or the CLI `bootstrap_root_password`) drives the fixed bootstrap through a
no-echo PTY and `su`; it is never reused for ordinary management operations.
Successful native deployments surface the installer's final summary (including
the authoritative Web URL) while omitting the noisy package transcript; every
connection secret is redacted before that summary reaches progress output.
Web-password rotation is opt-in and uses the same bounded, wiped stdin secret channel as
Wi-Fi management: no credential is placed in an SSH command or progress line.
The Deploy view leaves the generated credential in place unless
**Rotate the Web GUI password after deploy** is enabled; Manage and the CLI
`web-password` command perform the same explicit action later.
The fixed action invokes `omt-web set-password` inside the unprivileged
container and restarts OpenRC; `omt-web` validates the value and atomically writes
only a PBKDF2-SHA256 hash.
Renaming an installed appliance (`ops::set_hostname`, the terminal **Manage** view,
and CLI `hostname`) uploads `deploy/host/set-hostname.sh` the same way Alpine
setup uploads its own script, so a board deployed before the action existed can
still be renamed. The name is the *host's*: the Web GUI reads `/etc/hostname`,
and Docker fills a host-network container's copy of that file once, at creation
time, so the script recreates the container rather than restarting the process
inside it.
`install.sh` removes the appliance images its `docker load` supersedes, matching
on this product's OCI image title and confirming each candidate is untagged and
is not the image just loaded. Docker's `dangling=true` filter is not used: on
Alpine's engine, combining it with a label filter also returns the tagged
image.

## Legal and release

`LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `THIRD_PARTY_SOURCE.md` are release
inputs. `scripts/check-legal-notices.py` checks that every shipped third-party
component -- OpenSSL, the Rust standard library, the Unicode data, the static runtimes -- carries its notice. `scripts/generate-runtime-sbom.py` and
`scripts/generate-deployer-sbom.py` create CycloneDX inventories.

`make release` is the local release pipeline; there is no GitHub Actions
workflow. It requires a clean branch with an upstream and an authenticated
GitHub CLI, runs the ARM64 and both deployer publishers in dependency order,
creates an annotated tag from the canonical `VERSION` when needed,
atomically pushes the branch and tag, and creates a GitHub Release with
reproducible Linux and Windows package archives plus SHA-256 checksums. Existing
release assets are immutable: rerunning the command recognizes an existing
release and does not replace them.
