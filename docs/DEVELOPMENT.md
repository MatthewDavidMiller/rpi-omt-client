# Development

The entry point for anyone — human or agent — changing this repository. Read
[ARCHITECTURE.md](ARCHITECTURE.md) for why the runtime is shaped the way it is
and [CODEBASE_REFERENCE.md](CODEBASE_REFERENCE.md) for which file owns what.

## Workspace at a glance

A C17 tree, a shell deployment capsule, and a container image. OpenSSL is the
only third-party library; ALSA (on the appliance), the C library, the Linux
uapi, and Win32 are the platform.

| Directory | Owns |
|---|---|
| `src/common` | Bounded buffers, strict JSON and XML, NFC, file and process I/O, randomness |
| `src/protocol` | OMT wire transport and shared target validation |
| `src/vmx` | Decode-only VMX1, worker pool, AArch64 NEON kernels |
| `src/receiver_core` | Format policy, video ceilings, status projection |
| `src/receiver` | Linux adapters: DRM/KMS, ALSA, discovery, D-Bus, the `omt-receiver` binary |
| `src/sender` | First-party OMT A/V test sender |
| `src/crypto` | The OpenSSL EVP adapters the Web frontend and deployer share |
| `src/web` | HTTPS operator GUI (poll loop, OpenSSL TLS), templates, diagnostics |
| `src/deploy/core` | Deployment jobs, the platform layer, SD-card prep, workstation probes |
| `src/deploy/ssh` | First-party SSH and SFTP client on OpenSSL primitives |
| `src/deploy/capsule` | The embedded manifest-v3 capsule |
| `src/deploy/cli` | `rpi-omt-deploy`, human and JSON-lines surfaces |
| `src/deploy/tui` | `rpi-omt-deploy-tui`, the terminal deployer for Linux and Windows |
| `mk/` | The makefiles: flags, libraries, products |

| Directory | Owns |
|---|---|
| `deploy/` | Dockerfile, container scripts, host installer, OpenRC services, manifest-v3 capsule |
| `scripts/` | Build, gate, deploy, and release entry points |
| `tests/` | C suites (`tests/c`), fuzz targets (`tests/fuzz`), shell, Python, and native suites, schema vectors |
| `tools/gen/` | Build-time generators: NFC tables, templates, Blowfish tables, the capsule |
| `tools/toolbox/` | The image every gate runs inside |

Both deployer frontends run the same jobs from `src/deploy/core/jobs.c`, so a
deployment means one thing regardless of which one the operator ran. The
deployer ships as a CLI and a terminal application on both Linux and Windows:
a terminal frontend opens no graphics stack, so each is one self-contained
file, and it works over SSH.

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
make build-deployer           # Linux CLI + TUI, static-PIE musl
make build-windows-deployer   # Windows CLI + TUI, mingw-w64 with the pinned OpenSSL
make build-omt-sender         # OMT A/V test sender

# Verify
make test-web | test-receiver | test-deployer    # narrow suites (sanitized)
make test-c | fuzz-smoke      # every C suite under ASan+UBSan; every fuzz target
make test-quick               # every unit suite, no container engine (~1m)
make test                     # + Windows cross build, amd64 image, ARM64 builder stage
make lint                     # C static gates, supply chain, shell, docker, yaml, python, legal
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
a commit rather than shipping. C has no borrow checker, so the gates carry the
weight the compiler used to.

- **Bounded by default.** Every read, subprocess, allocation, retry, and rate
  limit carries an explicit ceiling. Buffers are `omt_buf`s created with the
  most they may ever hold; parsing goes through `omt_span` cursors whose every
  read is length-checked. Unbounded input handling is a defect here even where
  growth looks impossible.
- **The unsafe libc calls do not compile.** `src/common/banned.h` is
  force-included into every file and poisons `strcpy`, `sprintf`, `strtok`,
  `atoi`, `gets`, `alloca`, and the rest; `scripts/check-c.sh` refuses
  `rand`. Length arithmetic uses the checked `omt_add`/`omt_mul` helpers.
- **Warnings are errors, under both compilers.** `mk/flags.mk` builds with
  `-Werror -Wconversion -Wsign-conversion` and the rest, hardened
  (`-ftrivial-auto-var-init=zero`, stack protector and clash protection,
  `_FORTIFY_SOURCE=3`, full RELRO, PIE). `scripts/check-c.sh` builds with GCC
  and Clang at debug and release, runs the GCC static analyzer and cppcheck,
  and checks formatting and every generated source.
- **Every suite runs sanitized.** The C suites run under AddressSanitizer and
  UndefinedBehaviorSanitizer with leak detection; every parser that reads
  untrusted input has a libFuzzer target in `tests/fuzz`, and `make
  fuzz-smoke` runs them all.
- **No third-party code but OpenSSL.** `scripts/check-supply-chain.sh`
  allowlists every system header a source may include, pins the Windows
  OpenSSL by version and SHA-256 to the 3.5 LTS series, and requires every
  base image to be pinned by digest. `scripts/check-no-c-sources.sh` keeps C
  in `src/`, `tests/c`, and `tests/fuzz`.
- **The security posture is load-bearing.** HTTPS, authentication, CSRF, rate
  limiting, source-name validation, and the deployer's strict host-key and
  strict-key-exchange SSH are asserted by tests. Relaxing one is a deliberate
  design change, not a refactor.
- **One version.** `VERSION` is canonical, `scripts/detect-version.sh`
  resolves it, and the build stamps it into every binary. Artifacts carry the
  version of their commit, which is why publishing runs from
  `.githooks/post-commit` rather than the commit gate.
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
