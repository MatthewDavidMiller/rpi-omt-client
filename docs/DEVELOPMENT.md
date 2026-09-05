# Development

The entry point for anyone — human or agent — changing this repository. Read
[ARCHITECTURE.md](ARCHITECTURE.md) for why the runtime is shaped the way it is
and [CODEBASE_REFERENCE.md](CODEBASE_REFERENCE.md) for which file owns what.

## Workspace at a glance

A Rust 2024 workspace, a shell deployment capsule, and a container image.

| Crate | Owns |
|---|---|
| `crates/omt-protocol` | OMT wire transport and shared target validation |
| `crates/vmx-decoder` | Decode-only VMX1, worker pool, AArch64 NEON kernels |
| `crates/omt-receiver-core` | Format policy, sanitization, status projection |
| `crates/omt-receiver` | Linux adapters: DRM/KMS, ALSA, discovery, the `omt-receiver` binary |
| `crates/omt-web` | HTTPS operator GUI (Axum + rustls), diagnostics, host actions |
| `crates/omt-test-sender` | First-party OMT A/V test sender |
| `crates/omt-deployer-core` | Deployment jobs, SSH/SFTP, capsule, SD-card prep, workstation probes |
| `crates/rpi-omt-deploy` | Deployer CLI (human and JSON-lines surfaces) |
| `crates/rpi-omt-deploy-tui` | Linux terminal deployer, static musl |
| `crates/rpi-omt-deployer` | Windows egui deployer |

| Directory | Owns |
|---|---|
| `deploy/` | Dockerfile, container scripts, host installer, OpenRC services, manifest-v3 capsule |
| `scripts/` | Build, gate, deploy, and release entry points |
| `tests/` | Shell, Python, and native suites plus the shared schema vectors |
| `tools/toolbox/` | The image every gate runs inside |

Both deployer frontends run the same jobs from
`crates/omt-deployer-core/src/jobs.rs`, so a deployment means one thing
regardless of which one the operator ran. Linux gets a terminal frontend rather
than a GUI because egui `dlopen`s the operator's glibc-linked graphics driver,
which no single portable binary can carry; Windows keeps the GUI, where
`opengl32.dll` is a system library.

## Workstation contract

Docker or Podman — rootless Podman included — is the only thing a workstation
needs. Every gate runs inside `tools/toolbox/Dockerfile` through
`scripts/toolbox.sh`, which builds the image on first use and rebuilds it when a
pinned version changes.

```bash
make install    # build the toolbox image, point core.hooksPath at .githooks/
```

Never make a gate depend on a host-installed tool.
`scripts/install-dev-deps.sh` still provisions the toolchain for anyone who
wants it locally, but nothing requires it. See
[TESTING.md](TESTING.md) for the engine details and the no-skip rule.

## Commands

```bash
# Build
make build-arm64              # appliance image -> omt-client-arm64.tar.gz
make build-amd64              # local test image
make build-deployer           # Linux CLI + TUI, static musl
make build-windows-deployer   # mingw-w64 cross build
make build-omt-sender         # OMT A/V test sender

# Verify
make test-web | test-receiver | test-deployer    # narrow suites
make test-quick               # every unit suite, no container engine (~1m)
make test                     # + Windows cross build, amd64 image, ARM64 builder stage
make lint                     # rustfmt, clippy, supply chain, shell, docker, yaml, python, legal
make security-scan            # Trivy filesystem + image

# Run and ship
make up | down | logs         # local amd64 dev container
make deploy HOST=user@<ip>
make release                  # local pipeline; needs an authenticated gh CLI
```

Both deployer builds embed the appliance image, so `make build-arm64` comes
first; they stop and say so when `omt-client-arm64.tar.gz` is absent. It is
deliberately not a Make prerequisite, because an emulated ARM64 build takes tens
of minutes and should never start as a side effect.

## Invariants

These are enforced by gates, not by convention — a change that breaks one fails
a commit rather than shipping.

- **Bounded by default.** Every read, subprocess, allocation, retry, and rate
  limit carries an explicit ceiling. Unbounded input handling is a defect here
  even where growth looks impossible.
- **No unsafe.** `unsafe_code` is `forbid` workspace-wide. `vmx-decoder` alone
  downgrades it to `deny` for its AArch64 kernels, confining
  `#![allow(unsafe_code)]` to `crates/vmx-decoder/src/pool.rs`,
  `crates/vmx-decoder/src/convert/neon.rs`, and
  `crates/vmx-decoder/src/idct/neon.rs`, with `unsafe_op_in_unsafe_fn` denied.
- **Clippy `pedantic`, `unwrap_used`, and `expect_used` are `deny`** across the
  workspace; the allow-list in `Cargo.toml` is the whole exemption set.
- **No C or C++ sources, no Git dependencies, no unlocked registry packages.**
  `scripts/check-no-c-sources.sh` and `scripts/check-supply-chain.sh`
  (`cargo deny` and `cargo vet` over `deny.toml` and `supply-chain/`) enforce
  it; a new dependency needs a `Cargo.lock` entry and a cargo-vet record.
- **The security posture is load-bearing.** HTTPS, authentication, CSRF, rate
  limiting, and source-name validation are asserted by tests. Relaxing one is a
  deliberate design change, not a refactor.
- **One version.** `workspace.package.version` in `Cargo.toml` is canonical,
  `scripts/detect-version.sh` resolves it, and intra-workspace path
  dependencies must pin that exact version. Artifacts stamp it in at build time,
  which is why publishing runs from `.githooks/post-commit` rather than the
  commit gate.
- **Shared contracts move together.** `tests/schema/omt-target-vectors.json` and
  `tests/schema/playback-status-vectors.json` are asserted by both the receiver
  and Web suites, and `tests/unit/test_cross_file_invariants.py` pins constants
  one file computes and another consumes.
- **Documentation is part of the change.** `tests/unit/test_documentation.py`
  checks that documented settings, routes, and file-map paths still exist.

## Validation

Run the narrowest gate that covers what you touched, then broaden if the change
crosses a boundary; the table is in
[TESTING.md](TESTING.md#which-gate-to-run). Say explicitly which gates ran and
which did not.

Raspberry Pi hardware is not assumed to be present. DRM, ALSA, HDMI hotplug,
OpenRC boot ordering, nftables, live OMT media, and per-board decode ceilings
are hardware validation boundaries — QEMU models neither SoC — and their
checklist is at the end of [TESTING.md](TESTING.md).

## Documentation map

| Doc | Covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Runtime design and the reasoning behind each bound |
| [CODEBASE_REFERENCE.md](CODEBASE_REFERENCE.md) | File-to-responsibility map |
| [CONFIGURATION.md](CONFIGURATION.md) | Env vars, persistent files, HDMI, decode ceilings |
| [TESTING.md](TESTING.md) | Gates, hooks, release pipeline, hardware tier |
| [SETUP.md](SETUP.md) | Install, upgrade, uninstall, headless first boot |
| [OPERATIONS.md](OPERATIONS.md) | Dashboard, network, diagnostics, troubleshooting |
| [DIAGNOSTICS_BUNDLE.md](DIAGNOSTICS_BUNDLE.md) | Support-bundle ZIP contract |
| [OMT_TEST_SENDER.md](OMT_TEST_SENDER.md) | First-party test sender |
