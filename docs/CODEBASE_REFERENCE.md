# Codebase Reference

## Runtime

| Area | Files |
|---|---|
| Native receiver | `src/native/receiver/main.cpp` |
| OMT wire transport and validation | `src/native/omt/omt_wire.c`, `src/native/omt/include/omt/omt_wire.h` |
| Status projection and atomic publication | `src/native/receiver/playback_status.cpp` |
| Shared JSON string escaping (status and CLI output) | `src/native/receiver/json_text.hpp` |
| Discovery and bounded network channels | `src/native/receiver/discovery.cpp`, `src/native/receiver/omt_channel.cpp` |
| DRM/ALSA playback | `src/native/receiver/drm_output.cpp`, `src/native/receiver/alsa_output.cpp` |
| Audited VMX source | `third_party/omt/PROVENANCE.md`, `third_party/omt/libvmx` |
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
| Alpine OpenRC services | `deploy/openrc/`, `deploy/host/host-event-watcher.sh` |

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

`runtime-sha256.manifest` covers `/app`, `/usr/local/bin`, **and**
the installed `omt_client` package with its `.dist-info`, so the integrity
check still spans the application code after the move. The first-party wheel is
excluded from the third-party notice sweep and from the SBOM's PyPI components;
`tests/integration/test_docker_build.sh` asserts all of this.

The native receiver is built with CMake, Ninja, Clang, and LLD in a
digest-pinned Alpine builder stage. The compiler stays on amd64 and emits
ARM64 against a minimal Alpine target sysroot. A `scratch` stage contains only
the stripped native `omt-receiver`; the runtime supplies its small Alpine
ALSA, Avahi, DRM, C/C++ runtime dependency set and copies the executable from that
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
| Deployment contract | `deploy/manifest-v3.txt`, `deploy/transaction.sh` |
| CLI deployment | `scripts/deploy.sh` |
| Native deployer validation, models, and project-root discovery | `src/native/deployer/core.cpp`, `src/native/deployer/core.hpp` |
| Legal texts compiled into the deployer | `cmake/EmbedText.cmake`, `cmake/EmbedText.cpp.in`, `src/native/deployer/legal_texts.hpp` |
| Secure local process and SSH boundaries | `src/native/deployer/process.cpp`, `src/native/deployer/ssh_client.cpp` |
| Deployment/Wi-Fi operations | `src/native/deployer/deployment.cpp` |
| SDL3/ImGui presentation and About UI | `src/native/deployer/ui_main.cpp` |
| Hash-locked deployer dependencies | `cmake/NativeDependencies.cmake` |
| Windows cross build and artifact contract | `scripts/build-windows-deployer.sh`, `scripts/verify-windows-deployer.sh`, `cmake/toolchains/windows-x86_64-mingw.cmake` |
| Local toolchain provisioning | `scripts/install-dev-deps.sh`, `scripts/install-hadolint.sh`, `scripts/install-trivy.sh`, `scripts/install-arm64-emulation.sh` |

## Legal and release

`LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `THIRD_PARTY_SOURCE.md` are release
inputs. `scripts/check-legal-notices.py` compares shipped Python and native
dependencies to the notices. `scripts/generate-runtime-sbom.py` and
`scripts/generate-deployer-sbom.py` create CycloneDX inventories.
