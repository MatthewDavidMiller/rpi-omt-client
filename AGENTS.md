# AGENTS.md

This file provides guidance to Codex and other coding agents working in this repository.

## Project Overview

Raspberry Pi OMT Client receives OMT video/audio streams on a Raspberry Pi 5 and outputs them to HDMI. The system uses:

- A multi-stage Docker build for the NativeAOT .NET OMT receiver and `libvmx`
- A Flask web UI for authentication and OMT source selection
- Direct DRM/KMS video and ALSA audio playback inside the container

Target platform: Raspberry Pi 5, 64-bit Raspberry Pi OS Lite. Local development usually happens on amd64.

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
- Never add agent co-author trailers such as `Co-Authored-By` to commits.

## Restricted Files

- Treat `vars.yml` as sensitive user configuration. Do not read or modify it unless the user explicitly asks.

## Key Commands

```bash
# Build
make build-arm64
make build-amd64

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
sudo ./install.sh
```

## Validation Expectations

Run the narrowest relevant checks for the files you touched, then broaden if the change crosses subsystem boundaries.

- Flask app or templates: `make test-py` and, when behavior changes materially, `make test`
- Shell scripts or installer logic: `make test-quick`
- Dockerfile, entrypoint, or image contents: `make test` or `./scripts/test-local.sh --full` when feasible
- Documentation-only changes: no test run required, but keep commands and paths accurate

If you cannot run a relevant validation step, say so explicitly in your final handoff.

## High-Value File Map

- `app/omt_client/factory.py` and `app/omt_client/routes/`: Flask composition and public routes
- `app/omt_client/services.py`: injected domain boundary and playback/source operations
- `app/state_store.py`: bounded state and atomic OMT target persistence
- `app/templates/` and `app/static/`: web UI
- `omt/runtime-lib.sh`: shared shell validation, bounded reads, and process identity
- `omt/entrypoint.sh`: container bootstrap, secret/cert generation, gunicorn startup
- `omt/start-omt.sh`: validated native OMT receiver launcher
- `install.sh`: Pi install, Docker/bootstrap, HDMI config, systemd integration
- `Dockerfile`: cross-build and runtime image assembly
- `tests/unit/` and `tests/integration/`: main safety net

## Agent Handoff

When finishing work, report:

- What changed
- What validation ran
- Any constraints, skipped checks, or Pi-specific risks that remain
