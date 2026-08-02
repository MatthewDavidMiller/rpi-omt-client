# Codebase Reference

## Runtime

| Area | Files |
|---|---|
| Native receiver | `src/receiver/RpiOmt.Receiver/Program.cs` |
| Receiver policy/status core | `src/receiver/RpiOmt.Receiver.Core` |
| Status projection and publish throttle | `src/receiver/RpiOmt.Receiver.Core/PlaybackState.cs` |
| Receiver heartbeat waits and atomic status publication | `src/receiver/RpiOmt.Receiver.Core/InterruptibleWait.cs`, `src/receiver/RpiOmt.Receiver.Core/AtomicFilePublisher.cs` |
| HDMI connector selection | `src/receiver/RpiOmt.Receiver.Core/HdmiConnectors.cs` |
| Audited OMT source | `third_party/omt/PROVENANCE.md`, `third_party/omt/libomtnet`, `third_party/omt/libvmx`, `third_party/omt/omtplayer` |
| Flask composition | `src/omt_client/factory.py`, `src/omt_client/wsgi.py` |
| Service composition | `src/omt_client/services/composition.py` |
| Typed service protocols | `src/omt_client/services/protocols.py` |
| Authentication, playback, network, diagnostics, host system | `src/omt_client/services/` |
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
| Full Raspberry Pi OS VM lifecycle | `scripts/pi-os-vm.sh`, `scripts/pi-os-vm-toolbox.sh`, `scripts/install-pi-os-vm-tooling.sh`, `tests/vm/` |

`RuntimeAbout` is the single owner of the build version. The About page, the
diagnostics page, and the `version.txt` member of a support bundle all read it
through that one service, so an archive cannot name a different build than the
UI that produced it. `DiagnosticsService` therefore has no `version()` of its
own; `RuntimeDiagnostics` receives the About service instead. Build entry
points prefer an explicit `RPI_OMT_CLIENT_VERSION`, then the canonical version
in `pyproject.toml`, before falling back to release metadata from Git or a
versioned source directory.

`deploy/Dockerfile` builds a wheel from the `packages` list in `pyproject.toml`
and installs it into `/opt/venv`, so the appliance imports `omt_client` from
site-packages rather than a copied tree. That list names only `omt_client*`, so
`src/omt_client_preview/` (the in-memory fakes behind
`scripts/preview-web-ui.py` and the route tests) never reaches the appliance
image. Nothing under `src/omt_client/` may import it;
`tests/unit/test_preview_services.py` enforces that.

`runtime-sha256.manifest` covers `/app`, `/usr/local/bin`, `libvmx.so`, **and**
the installed `omt_client` package with its `.dist-info`, so the integrity
check still spans the application code after the move. The first-party wheel is
excluded from the third-party notice sweep and from the SBOM's PyPI components;
`tests/integration/test_docker_build.sh` asserts all of this.

The native receiver is built in the digest-pinned Alpine NativeAOT SDK stage.
The compiler stays on amd64 and emits ARM64 against a minimal Alpine target
sysroot, avoiding failures in the ARM64-hosted .NET 10 ILC process. The build
step then removes NuGet and compiler intermediates. A `scratch` stage
contains only `omt-receiver` and `libvmx.so`; the runtime copies from that
stage, so neither the SDK nor build toolchain is part of the deployed image.

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
| Deployment contract | `deploy/manifest-v2.txt`, `deploy/transaction.sh` |
| CLI deployment | `scripts/deploy.sh` |
| Windows models/validation | `src/deployer/RpiOmt.Deployer.Core/Models.cs` |
| Windows action state | `src/deployer/RpiOmt.Deployer.Core/ActionController.cs` |
| Windows deployment facade | `src/deployer/RpiOmt.Deployer.Core/DeploymentOperations.cs` |
| Windows view models | `src/deployer/RpiOmt.Deployer.App/ViewModels/MainViewModel.cs`, `src/deployer/RpiOmt.Deployer.App/ViewModels/SectionViewModels.cs` |
| Windows About UI | `src/deployer/RpiOmt.Deployer.App/BuildInformation.cs`, `src/deployer/RpiOmt.Deployer.App/Views/MainWindow.axaml` |
| Windows theme and control styles | `src/deployer/RpiOmt.Deployer.App/App.axaml`, `src/deployer/RpiOmt.Deployer.App/Styles/Tokens.axaml`, `src/deployer/RpiOmt.Deployer.App/Styles/Controls.axaml` |

## Legal and release

`LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `THIRD_PARTY_SOURCE.md` are release
inputs. `scripts/check-legal-notices.py` compares shipped Python and Windows
dependencies to the notices. `scripts/generate-runtime-sbom.py` and
`scripts/generate-windows-sbom.py` create CycloneDX inventories.
