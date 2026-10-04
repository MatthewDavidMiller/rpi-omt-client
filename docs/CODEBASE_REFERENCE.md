# Codebase Reference

## Runtime

| Area | Files |
|---|---|
| Receiver CLI and command dispatch | `src/receiver/main.c` |
| OMT TCP channel, subscription, and bounded frame reads | `src/receiver/channel.c` |
| Source discovery (central server, then Avahi over D-Bus) | `src/receiver/discovery.c`, `src/receiver/mdns_dbus.c` |
| First-party D-Bus wire client: SASL EXTERNAL, marshalling, bounded messages | `src/receiver/dbus.c` |
| Bounded XML reads for settings and announcements | `src/common/xml.c` |
| HDMI connector selection and hotplug checks | `src/receiver/connector.c` |
| Direct KMS scanout and mode selection, raw ioctls against an in-tree uapi | `src/receiver/video_drm.c`, `src/receiver/drm_uapi.h` |
| Aspect-preserving resample into a mode that is not the video's size | `src/receiver/scale.c` |
| HDMI audio through ALSA | `src/receiver/audio_alsa.c`, `src/receiver/audio_interleave.c` |
| Compressed A/V playout queue | `src/receiver/jitter.c` |
| Playback supervisor, retry, and audio worker | `src/receiver/play.c` |
| OMT wire transport and validation | `src/protocol/omt.c` |
| Video ceilings, status projection, and atomic publication | `src/receiver_core/ceiling.c`, `src/receiver_core/status.c` |
| Decode-only VMX1 implementation and its worker pool | `src/vmx/` |
| VMX bitstream and plane decoding | `src/vmx/bitstream.h`, `src/vmx/plane.c` |
| Entropy lookahead table and its generator | `src/vmx/ac_lookahead.c`, `tools/gen/gen_vmx_tables.py` |
| Colour conversion: portable definition and AArch64 assembly BGRX kernel | `src/vmx/convert_scalar.c`, `src/vmx/convert_aarch64.S` |
| Inverse DCT: portable definition and AArch64 assembly kernel | `src/vmx/idct_scalar.c`, `src/vmx/idct_aarch64.S` |
| Branch-protection landing pads and property note for the assembly | `src/vmx/aarch64.inc` |
| Decode throughput measurement for per-board ceilings | `tests/c/bench_vmx.c` |
| VMX conformance vectors captured from the reference decoder | `tests/vectors/vmx/` |
| First-party C OMT A/V test sender | `src/sender/`, `scripts/build-omt-test-sender.sh`, `scripts/omt-test-sender.sh` |
| Source-scoped sender firewall helper | `scripts/configure-omt-test-sender-firewall.sh` |
| Shared validation, status, and forbidden-code-point contracts | `tests/schema/` |
| Bounded buffers, strict JSON, NFC, file and process I/O | `src/common/` |
| The build version, stamped into this one object so a version change recompiles one file | `src/common/version.c` |
| NFC tables generated from the pinned Unicode data | `tools/gen/gen_nfc_tables.py`, `third_party/unicode/` |
| OMT provenance | `third_party/omt/PROVENANCE.md`, `third_party/omt/libvmx/LICENSE.txt` |
| HTTPS server: poll loop, OpenSSL TLS, request parsing, connection and time limits | `src/web/http.c` |
| Routes, headers, CSRF, sessions, and rate limits | `src/web/app.c` |
| Credentials, legacy hash compatibility, and persistent sessions | `src/web/auth.c` |
| OpenSSL EVP adapters: hashes, HMAC, PBKDF2, scrypt | `src/crypto/crypto.c` |
| Source discovery, playback, and status projection | `src/web/playback.c` |
| Diagnostics, support archives, PCAP validation, and host actions | `src/web/diagnostics.c`, `src/web/zip.c` |
| Safe bounded/atomic I/O | `src/common/fsio_posix.c` |
| Persistent source, video-limit, and playout-delay state | `src/web/state.c` |
| OMT discovery-server XML | `src/web/network.c` |
| Validated runtime configuration | `src/web/settings.c` |
| Bounded subprocess execution | `src/common/proc_posix.c` |
| Templates (compiled to C by `tools/gen/gen_templates.py`) and design assets | `src/web/templates/`, `src/web/static/` |
| Shell process lifecycle | `deploy/container/runtime-lib.sh`, `deploy/container/start-omt.sh`, `deploy/container/control-omt.sh`, `deploy/container/entrypoint.sh` |
| Container | `deploy/Dockerfile`, `deploy/compose.yml` |
| Alpine OpenRC services | `deploy/openrc/`, `deploy/host/host-event-watcher.sh` |

The Web service has one version reader used by About, Diagnostics, and the
`version.txt` support-bundle member. Build entry points prefer an explicit
`RPI_OMT_CLIENT_VERSION`, then the canonical version in `VERSION`,
before falling back to release metadata from Git or a versioned source directory.
Because that version is baked into every artifact at build time, the builds that
publish one run from `.githooks/post-commit` rather than the commit gate.

`diagnostics.c` owns both sides of the correlated host request boundary and
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
`tests/schema/omt-target-vectors.json`. `src/protocol/omt.c` owns the compiled table
and both binaries link that module, so a name the receiver would play
cannot be one the dashboard silently drops.

Playback states are pinned the same way, in
`tests/schema/playback-status-vectors.json`. Every state in it is reachable:
the receiver suite asserts `video_states` is exactly what `video_name` produces,
and the Web tests assert every receiver state has a public projection.

The runtime is built from the C tree with Alpine's toolchain and OpenSSL in a
digest-pinned Alpine builder stage. A `scratch` stage contains only the
stripped `omt-receiver` and `omt-web`, so neither the SDK nor build toolchain is
part of the deployed image.
The ARM64 publisher fingerprints both binaries' complete local source closure
into that scratch stage: Podman's cross-stage cache can otherwise compile a
changed receiver and still reuse the old copied binary.
`src/deploy/` is excluded from the build context, so a deployer or SSH-client
source edit does not trigger another ARM64 receiver compile. That compile is a
cross-compile on the build machine against an Alpine sysroot
(`scripts/make-sysroot.sh`); only the runtime stage's package installs run
under emulation.

Public routes are `/login`, `/logout`, `/`, `/sources/select`,
`/sources/refresh`, `/playback/restart`, `/playback/clear`,
`/settings/network`, `/settings/direct-source`, `/diagnostics`,
`/diagnostics/discovery`, `/diagnostics/runtime`, `/diagnostics/direct`,
`/diagnostics/download`, `/system`, `/system/video-limit`, `/system/playout-delay`, `/system/reboot`, and `/about`.
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
| Capsule embedded into the deployer, and the generator that compiles it in with `.incbin` | `src/deploy/capsule/capsule.c`, `tools/gen/gen_capsule.py` |
| CLI deployment | `scripts/deploy.sh` |
| Deployer validation, fixed actions, deploy, Alpine sys setup, rename, and Wi-Fi | `src/deploy/core/core.c`, `src/deploy/core/ops.c` |
| First-party SSH client: transport, key exchange (`src/deploy/ssh/kex.c`), host keys and `known_hosts`, user keys and bcrypt_pbkdf, authentication, channels, SFTP | `src/deploy/ssh/` |
| Deployer platform layer for Linux and Windows | `src/deploy/core/sys_posix.c`, `src/deploy/core/sys_win32.c` |
| Local Alpine SD-card preparation, verified headless-overlay download over a bounded HTTPS client, and initial Wi-Fi configuration | `src/deploy/core/sd_card.c`, `src/deploy/core/https.c` |
| Workstation tooling: executable discovery, prerequisites, winget installs, ARM64 emulation, and the image-build plan | `src/deploy/core/tools.c` |
| Secure command-line deployer | `src/deploy/cli/main.c` |
| Deployer CLI contract, and the SSH client held to OpenSSH | `tests/native/test_deployer_cli.sh`, `tests/integration/test_ssh_client.sh` |
| Terminal deployer for Linux and Windows: views and input handling (`src/deploy/tui/app.c`), a diffing VT renderer, and the embedded legal texts | `src/deploy/tui/` |
| Job definitions both deployer frontends share, so a deployment means one thing | `src/deploy/core/jobs.c` |
| Gate toolbox image: every compiler, linter, and scanner the gates use | `tools/toolbox/Dockerfile` |
| Target sysroot for the cross-compiled appliance build and AArch64 suites | `scripts/make-sysroot.sh` |
| AArch64 suites: cross-compiled, run under qemu-user | `scripts/test-c-arm64.sh` |
| Runs a gate inside the toolbox; the only thing the host needs is Docker or Podman | `scripts/toolbox.sh` |
| Reads the Linux deployer's static-linkage guarantee back out of the ELF headers | `scripts/verify-linux-deployer.sh` |
| Supply-chain gate and the pinned Windows OpenSSL | `scripts/check-supply-chain.sh`, `scripts/build-openssl.sh` |
| Windows cross build | `scripts/build-windows-deployer.sh` |
| Local commit gate, publishing builds, and GitHub Release publisher | `.githooks/pre-commit`, `.githooks/post-commit`, `scripts/setup-hooks.sh`, `scripts/publish-github-release.sh` |
| Local toolchain provisioning | `scripts/install-dev-deps.sh`, `scripts/install-hadolint.sh`, `scripts/install-trivy.sh`, `scripts/install-arm64-emulation.sh` |

`tools.c` is where the deployer's answers about the *operator's* machine live,
as opposed to `ops.c`, which is about the Pi. Every rule in it is a pure
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
Renaming an installed appliance (`dp_set_hostname`, the terminal **Manage** view,
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
component -- OpenSSL, the Unicode data, the static runtimes -- carries its notice. `scripts/generate-runtime-sbom.py` and
`scripts/generate-deployer-sbom.py` create CycloneDX inventories.

`make release` is the local release pipeline; there is no GitHub Actions
workflow. It requires a clean branch with an upstream and an authenticated
GitHub CLI, runs the ARM64 and both deployer publishers in dependency order,
creates an annotated tag from the canonical `VERSION` when needed,
atomically pushes the branch and tag, and creates a GitHub Release with
reproducible Linux and Windows package archives plus SHA-256 checksums. Existing
release assets are immutable: rerunning the command recognizes an existing
release and does not replace them.
