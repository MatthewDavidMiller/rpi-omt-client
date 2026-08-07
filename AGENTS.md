# AGENTS.md

This file provides guidance to Codex and other coding agents working in this repository.

## Project Overview

Raspberry Pi OMT Client receives OMT video/audio streams on a Raspberry Pi 5 and outputs them to HDMI. The system uses:

- A multi-stage Docker build for the Rust 2024 OMT receiver and VMX1 decoder
- A Flask web UI for authentication and OMT source selection
- Direct DRM/KMS video and ALSA audio playback inside the container

Target platform: Raspberry Pi 5, Alpine Linux 3.23 aarch64 in persistent sys
mode. Raspberry Pi OS and Alpine diskless mode are unsupported. Local
development usually happens on amd64.

## Read First

Before making code changes or navigating deeper into the repo, read these documents first:

1. `docs/ARCHITECTURE.md`
2. `docs/CODEBASE_REFERENCE.md`

Use these as the primary map of the system. Then pull in task-specific docs as needed:

- `docs/CONFIGURATION.md` for env vars, build args, paths, and HDMI config
- `docs/TESTING.md` for validation commands and local CI behavior
- `docs/SETUP.md` for Pi install, upgrade, uninstall, and first access
- `docs/OPERATIONS.md` for source/network control, diagnostics, and troubleshooting

## Working Rules

- Preserve the current security posture: HTTPS stays enabled, auth/CSRF/rate-limiting stay intact, and source-name validation must not be relaxed without a clear reason.
- Prefer minimal, targeted changes. This repo already has strong test coverage around the Flask app, entrypoint, install flow, and container behavior.
- Do not assume Raspberry Pi hardware is available locally. Prefer unit tests and amd64 container validation unless the task explicitly requires Pi-only verification.
- If you change documented behavior, update the relevant docs in the same pass.
- Never add co-authors to commits. Do not include `Co-Authored-By`,
  `Signed-off-by` for an agent, or any other trailer that attributes the
  commit to an AI/agent or additional author. Commits are authored only by
  the human operator.

## Restricted Files

- Treat `vars.yml` as sensitive user configuration. Do not read or modify it unless the user explicitly asks.

## Key Commands

```bash
# Setup (provisions every gate tool; fails if one is missing)
make install

# Build
make build-arm64
make build-amd64
make build-deployer
make build-windows-deployer   # mingw-w64 cross build, Linux host

# Local dev / preview
make up
make down
make logs
python3 scripts/preview-web-ui.py

# Tests
make test-setup
make test-quick
make test
./scripts/test-local.sh --full
make test-py
make lint

# Deploy
make deploy HOST=pi@<ip>
sudo ./install.sh   # on the Pi, from the deployed capsule
```

## Validation Expectations

Run the narrowest relevant checks for the files you touched, then broaden if the change crosses subsystem boundaries.

- Flask app or templates: `make test-py` and, when behavior changes materially, `make test`
- Shell scripts or installer logic: `make test-quick`
- Receiver core or the playback-status contract: `make test-receiver` and `make test-py`; both
  suites assert against `tests/schema/playback-status-vectors.json` and must be updated together
- Dockerfile, entrypoint, or image contents: `make test` or `./scripts/test-local.sh --full` when feasible
- Deployer core, SSH, or UI: `make test-deployer` and `make build-windows-deployer`, since the
  same sources ship as a host package and a cross-built Windows package
- Documentation-only changes: no test run required, but keep commands and paths accurate

If you cannot run a relevant validation step, say so explicitly in your final handoff.

No gate has a skip mode. A missing linter, scanner, cross toolchain, or ARM64
emulator fails its gate; repair the workstation with `make install` rather than
narrowing what runs.

## High-Value File Map

- `src/omt_client/factory.py` and `src/omt_client/routes/`: Flask composition and public routes
- `src/omt_client/services/`: injected domain boundary and playback/source operations
- `src/omt_client/services/composition.py`: production composition root
- `src/omt_client/services/about.py`: version and legal texts for About
- `src/omt_client/state_store.py`: bounded state and atomic OMT target persistence
- `src/omt_client/templates/` and `src/omt_client/static/`: web UI
- `src/omt_client_preview/`: dev-only in-memory fakes; deliberately outside the shipped package
- `crates/omt-protocol/`, `crates/vmx-decoder/`, and `crates/omt-receiver-core/`: bounded wire parsing, decoding, playback, and status
- `crates/omt-receiver/`: Linux receiver adapters and the `omt-receiver` binary
- `crates/omt-deployer-core/`, `crates/rpi-omt-deploy/`, and `crates/rpi-omt-deployer/`: shared deployment core, CLI, and egui desktop application
- `deploy/container/runtime-lib.sh`: shared shell validation, bounded reads, and process identity
- `deploy/container/entrypoint.sh`: container bootstrap, secret/cert generation, gunicorn startup
- `deploy/container/start-omt.sh`: validated native OMT receiver launcher
- `deploy/host/install.sh`: Alpine/Pi preflight, hardening, firmware, Docker, HDMI, and OpenRC integration
- `deploy/openrc/`: fixed OpenRC service definitions
- `deploy/Dockerfile`: cross-build and runtime image assembly
- `tests/unit/` and `tests/integration/`: main safety net
- `tests/schema/`: vectors shared by the Python and native suites

## Agent Handoff

When finishing work, report:

- What changed
- What validation ran
- Any constraints, skipped checks, or Pi-specific risks that remain
