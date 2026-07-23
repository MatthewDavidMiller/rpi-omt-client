# Codebase Reference

## Runtime

| Area | Files |
|---|---|
| Native receiver | `receiver/RpiOmt.Receiver/Program.cs` |
| Audited OMT source | `third_party/omt/PROVENANCE.md`, `third_party/omt/libomtnet`, `third_party/omt/libvmx`, `third_party/omt/omtplayer` |
| Flask composition | `app/omt_client/factory.py`, `app/omt_client/wsgi.py` |
| Service boundary | `app/omt_client/services.py` |
| Routes | `app/omt_client/routes/` |
| Persistent state | `app/state_store.py` |
| Discovery and URI validation | `app/discovery.py` |
| OMT XML settings | `app/network_config.py` |
| Templates and CSS | `app/templates/`, `app/static/` |
| Shell process lifecycle | `omt/runtime-lib.sh`, `omt/start-omt.sh`, `omt/control-omt.sh`, `omt/entrypoint.sh` |
| Container | `Dockerfile`, `docker-compose.yml` |

Public routes are `/login`, `/logout`, `/`, `/sources/select`,
`/sources/refresh`, `/playback/restart`, `/playback/clear`,
`/settings/network`, `/settings/direct-source`, `/diagnostics`,
`/diagnostics/discovery`, `/diagnostics/runtime`, `/diagnostics/direct`,
`/diagnostics/download`, `/debug`, `/system`, `/system/reboot`, and `/about`.
All routes other than login require a current persistent session. Mutations
are POST and CSRF protected.

## Host and deployment

| Area | Files |
|---|---|
| Installer/uninstaller | `install.sh`, `uninstall.sh` |
| Host diagnostics | `host-debug.sh` |
| Reboot validator | `host-reboot.sh` |
| Deployment contract | `deploy-artifacts.txt`, `deploy-transaction.sh` |
| CLI deployment | `scripts/deploy.sh` |
| Windows models/validation | `deployer/RpiOmt.Deployer.Core/Models.cs` |
| Windows action state | `deployer/RpiOmt.Deployer.Core/ActionController.cs` |
| Windows deployment | `deployer/RpiOmt.Deployer.Core/DeploymentOperations.cs` |
| Windows About UI | `deployer/RpiOmt.Deployer.App/BuildInformation.cs`, `deployer/RpiOmt.Deployer.App/Views/MainWindow.axaml` |

## Legal and release

`LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `THIRD_PARTY_SOURCE.md` are release
inputs. `scripts/check-legal-notices.py` compares shipped Python and Windows
dependencies to the notices. `scripts/generate-runtime-sbom.py` and
`scripts/generate-windows-sbom.py` create CycloneDX inventories.
