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
| HDMI audio through ALSA | `crates/omt-receiver/src/audio.rs` |
| Playback supervisor, retry, and audio worker | `crates/omt-receiver/src/play.rs` |
| OMT wire transport and validation | `crates/omt-protocol/src/lib.rs` |
| Status projection and atomic publication | `crates/omt-receiver-core/src/lib.rs` |
| Decode-only VMX1 implementation | `crates/vmx-decoder/` |
| VMX bitstream and colour conversion | `crates/vmx-decoder/src/bitstream.rs`, `crates/vmx-decoder/src/convert.rs` |
| Inverse DCT: portable definition, AArch64 NEON kernel, and the choice between them | `crates/vmx-decoder/src/idct/scalar.rs`, `crates/vmx-decoder/src/idct/neon.rs`, `crates/vmx-decoder/src/idct/mod.rs` |
| VMX conformance vectors captured from the reference decoder | `tests/vectors/vmx/` |
| First-party Rust OMT A/V test sender | `crates/omt-test-sender/`, `scripts/build-omt-test-sender.sh`, `scripts/omt-test-sender.sh` |
| Source-scoped sender firewall helper | `scripts/configure-omt-test-sender-firewall.sh` |
| Shared validation, status, and forbidden-code-point contracts | `tests/schema/` |
| Per-artifact SBOM closures from `Cargo.lock` | `scripts/cargo_lock.py` |
| OMT provenance | `third_party/omt/PROVENANCE.md`, `third_party/omt/libvmx/LICENSE.txt` |
| Flask composition | `src/omt_client/factory.py`, `src/omt_client/wsgi.py` |
| Service composition | `src/omt_client/services/composition.py` |
| Typed service protocols | `src/omt_client/services/protocols.py` |
| Authentication, playback, network, host system | `src/omt_client/services/` |
| Diagnostics, split by trust boundary | `src/omt_client/services/diagnostics/` |
| Container-side checks and their JSON archive members | `src/omt_client/services/diagnostics/checks.py` |
| Correlated host request channel and PCAP validation | `src/omt_client/services/diagnostics/host.py` |
| `RuntimeDiagnostics` and the support-archive layout | `src/omt_client/services/diagnostics/bundle.py` |
| Routes | `src/omt_client/routes/` |
| Safe I/O and persistent state | `src/omt_client/safe_io.py`, `src/omt_client/state_store.py` |
| Shared host `key=value` record parsing | `src/omt_client/records.py` |
| Strict schema-bound JSON parsing | `src/omt_client/json_document.py` |
| Receiver status contract (consumer half) | `src/omt_client/playback_status.py` |
| Discovery and URI validation | `src/omt_client/discovery.py` |
| Shared ASCII host grammar | `src/omt_client/hostnames.py` |
| OMT XML settings | `src/omt_client/network_config.py` |
| Templates and CSS | `src/omt_client/templates/`, `src/omt_client/static/` |
| About route (presentation) | `src/omt_client/routes/about.py` |
| Build version and legal texts (service) | `RuntimeAbout` in `src/omt_client/services/about.py` |
| Web design tokens and layout | `src/omt_client/static/style.css`, `src/omt_client/static/favicon.svg` |
| Dev-only preview fakes | `src/omt_client_preview/` |
| Shell process lifecycle | `deploy/container/runtime-lib.sh`, `deploy/container/start-omt.sh`, `deploy/container/control-omt.sh`, `deploy/container/entrypoint.sh` |
| Container | `deploy/Dockerfile`, `deploy/compose.yml` |
| Alpine OpenRC services | `deploy/openrc/`, `deploy/host/host-event-watcher.sh` |

`RuntimeAbout` is the single owner of the build version. The About page, the
diagnostics page, and the `version.txt` member of a support bundle all read it
through that one service, so an archive cannot name a different build than the
UI that produced it. `DiagnosticsService` therefore has no `version()` of its
own; `RuntimeDiagnostics` receives the About service instead. Build entry
points prefer an explicit `RPI_OMT_CLIENT_VERSION`, then the canonical version
in `pyproject.toml`, before falling back to release metadata from Git or a
versioned source directory.

`src/omt_client/services/diagnostics/` is split by trust boundary rather than
by type:
`checks.py` runs only what the container can answer itself, `host.py` owns the
correlated request channel to the privileged collector, and `bundle.py`
composes both and lays out the zip. Members reach `bundle.py` already resolved,
including their stated failure reasons, so the archive layout decides nothing
about content. `RuntimeDiagnostics` remains the only name outside the package.
Its `runtime()` returns the check *and* the controller status that check
observed, so the diagnostics page header cannot contradict the check rendered
beneath it.

`deploy/Dockerfile` builds a wheel from the `packages` list in `pyproject.toml`
and installs it into `/opt/venv`, so the appliance imports `omt_client` from
site-packages rather than a copied tree. That list names only `omt_client*`, so
`src/omt_client_preview/` (the in-memory fakes behind
`scripts/preview-web-ui.py` and the route tests) never reaches the appliance
image. Nothing under `src/omt_client/` may import it;
`tests/unit/test_preview_services.py` enforces that.

`runtime-sha256.manifest` covers `/app`, `/usr/local/bin`, **and**
the installed `omt_client` package with its `.dist-info`, so the integrity
check still spans the application code after the move. The first-party wheel is
excluded from the third-party notice sweep and from the SBOM's PyPI components;
`tests/integration/test_docker_build.sh` asserts all of this.

A source name's forbidden code points have one published definition, in
`tests/schema/omt-target-vectors.json`: the Python validator derives them from
`unicodedata` and `omt-protocol` compiles them into a table it asserts against
that file. Both suites read it, so a name the receiver would play cannot be one
the dashboard silently drops.

Playback states are pinned the same way, in
`tests/schema/playback-status-vectors.json`. Every state in it is reachable:
the Rust suite asserts `video_states` is exactly what `video_name` produces and
the Python suite asserts `PUBLIC_STATES` is total over `receiver_states`, so a
state cannot be added on one side alone, nor left behind once no producer emits
it.

The receiver is built with checksum-locked Cargo dependencies and Rust 1.97.1
in a digest-pinned Alpine builder stage. A `scratch` stage contains only the
stripped `omt-receiver`, so neither the SDK nor build toolchain is part of the
deployed image.
The ARM64 publisher fingerprints the receiver's complete local source closure
into that scratch stage: Podman's cross-stage cache can otherwise compile a
changed receiver and still reuse the old copied binary.
The builder copies only the four receiver-side crates plus the deployer
manifests and inert target stubs needed to preserve the locked workspace, so a
desktop or SSH-client source edit does not trigger another emulated ARM64
receiver compile.

Public routes are `/login`, `/logout`, `/`, `/sources/select`,
`/sources/refresh`, `/playback/restart`, `/playback/clear`,
`/settings/network`, `/settings/direct-source`, `/diagnostics`,
`/diagnostics/discovery`, `/diagnostics/runtime`, `/diagnostics/direct`,
`/diagnostics/download`, `/system`, `/system/reboot`, and `/about`.
All routes other than login require a current persistent session. Mutations
are POST and CSRF protected.

## Host and deployment

| Area | Files |
|---|---|
| Installer/uninstaller | `deploy/host/install.sh`, `deploy/host/uninstall.sh` |
| Host diagnostics | `deploy/host/host-diagnostics.sh` |
| Reboot validator | `deploy/host/host-reboot.sh`, `deploy/lib/reboot-request.sh` |
| Shared host helpers | `deploy/lib/host-validation.sh`, `deploy/lib/publication.sh`, `deploy/lib/service-install.sh` |
| HDMI boot-configuration rules | `deploy/lib/hdmi-config.sh` |
| Supported boards and decode ceilings | `deploy/lib/board-profile.sh` |
| Deployment contract | `deploy/manifest-v3.txt`, `deploy/transaction.sh` |
| CLI deployment | `scripts/deploy.sh` |
| Deployer validation, fixed actions, SSH/SFTP, deploy, and Wi-Fi | `crates/omt-deployer-core/src/lib.rs`, `crates/omt-deployer-core/src/ssh.rs`, `crates/omt-deployer-core/src/ops.rs` |
| Workstation tooling: executable discovery, prerequisites, winget installs, and the image-build plan | `crates/omt-deployer-core/src/tools.rs` |
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
separately. Privileged remote operations use the provided, zeroized sudo
credential for non-root accounts and run directly for root. The SSH command
adapter continues reading through an EOF notification so that the server's
subsequent exit status remains authoritative.
On a clean Alpine host with no usable escalation rule, a separately zeroized
initial root password drives the fixed bootstrap through a no-echo PTY and
`su`; it is never reused for ordinary management operations.
Successful native deployments surface the installer's final summary (including
the authoritative Web URL) while omitting the noisy package transcript; every
connection secret is redacted before that summary reaches progress output.

## Legal and release

`LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `THIRD_PARTY_SOURCE.md` are release
inputs. `scripts/check-legal-notices.py` compares shipped Python and native
dependencies to the notices. `scripts/generate-runtime-sbom.py` and
`scripts/generate-deployer-sbom.py` create CycloneDX inventories.
