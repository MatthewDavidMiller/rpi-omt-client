# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Raspberry Pi OMT Client — receives Open Media Transport video/audio streams on a Raspberry Pi 5 and outputs them to HDMI. Uses bounded Rust 2024 receiver and HTTPS Web services with direct DRM/KMS video and ALSA audio. Managed via Docker; includes a Rust CLI and an egui deployer.

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
# Build the gate toolbox image (Docker or Podman is the only host dependency)
make install

# Build ARM64 image (for Raspberry Pi 5)
make build-arm64         # → omt-client-arm64.tar.gz

# Build the operator deployment application
make build-deployer            # Linux CLI + TUI, static musl, runs on any distro
make build-windows-deployer    # Windows x86-64 CLI + egui GUI (cross build)

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
pytest case fails the run. Every gate runs inside `tools/toolbox/Dockerfile`
via `scripts/toolbox.sh`; rebuild it with `make install`.

The Linux deployer is a terminal application, not a GUI. egui `dlopen`s the
operator's glibc-linked graphics driver, so a Linux GUI cannot be the
one-binary-runs-anywhere artifact this ships; Windows keeps the egui GUI, where
`opengl32.dll` is a system library. Both frontends run the same jobs from
`crates/omt-deployer-core/src/jobs.rs`.

## Architecture

See `docs/ARCHITECTURE.md` for the runtime stack and container/host boundaries.

## Key Files

See `docs/CODEBASE_REFERENCE.md` for the responsibility map.

## Git

Never add Claude as a co-author in git commit messages. Do not include `Co-Authored-By` trailers.

## Security

`vars.yml` is gitignored if it exists. Claude is denied read access to it via `.claude/settings.json`.
