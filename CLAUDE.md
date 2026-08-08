# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Raspberry Pi OMT Client — receives Open Media Transport video/audio streams on a Raspberry Pi 5 and outputs them to HDMI. Uses a bounded Rust 2024 receiver with direct DRM/KMS video and ALSA audio. Managed via Docker; includes a Flask web UI, a Rust CLI, and an egui deployer.

**Target:** Raspberry Pi 5 running Alpine Linux 3.24 aarch64 in persistent sys mode. Raspberry Pi OS and Alpine diskless mode are unsupported.

## Documentation

**Read `docs/ARCHITECTURE.md` and `docs/CODEBASE_REFERENCE.md` first** before navigating source files.

| File | Description |
|------|-------------|
| `README.md` | Quick start and links |
| `docs/SETUP.md` | Install, upgrade, uninstall, and first access |
| `docs/OPERATIONS.md` | Dashboard, network settings, diagnostics, and troubleshooting |
| `docs/ARCHITECTURE.md` | Runtime architecture and receiver/container boundaries |
| `docs/CONFIGURATION.md` | All build args, environment variables, volume paths, HDMI config |
| `docs/CODEBASE_REFERENCE.md` | File map and responsibility index |
| `docs/DIAGNOSTICS_BUNDLE.md` | Support-bundle ZIP contract |
| `docs/TESTING.md` | Testing, linters, git hooks |

## Essential Commands

```bash
# Provision every local gate tool (compilers, linters, Trivy, mingw-w64, ARM64 emulation)
make install

# Build ARM64 image (for Raspberry Pi 5)
make build-arm64         # → omt-client-arm64.tar.gz

# Build the operator deployment application
make build-deployer            # host-native package
make build-windows-deployer    # Windows x86-64 cross build (Linux host)

# Deploy to Pi
make deploy HOST=pi@<ip>   # promote capsule + run deploy/host/install.sh

# On the Pi
sudo ./deploy/host/install.sh  # Hardening, firmware, Docker, HDMI, OpenRC

# Local testing (amd64)
make build-amd64           # Build local test image
make up                    # Start local container on port 5000
make test                  # Unit + Docker build tests
./scripts/test-local.sh --full   # Also runs container smoke tests
./scripts/test-local.sh --quick  # Unit tests only (no Docker)

# Lint
make lint                  # shellcheck + hadolint + yamllint + ruff + mypy
```

No gate skips: a missing tool, an unregistered ARM64 emulator, or a skipped
pytest case fails the run. Repair the workstation with `make install`.

## Architecture

See `docs/ARCHITECTURE.md` for the runtime stack and container/host boundaries.

## Key Files

See `docs/CODEBASE_REFERENCE.md` for the responsibility map.

## Git

Never add Claude as a co-author in git commit messages. Do not include `Co-Authored-By` trailers.

## Security

`vars.yml` is gitignored if it exists. Claude is denied read access to it via `.claude/settings.json`.
