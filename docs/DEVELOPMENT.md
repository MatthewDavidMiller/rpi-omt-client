# Development

The entry point for anyone — human or agent — changing this repository. Read
[ARCHITECTURE.md](ARCHITECTURE.md) for why the runtime is shaped the way it is
and [CODEBASE_REFERENCE.md](CODEBASE_REFERENCE.md) for which file owns what.

## Workspace at a glance

A Rust workspace, a shell deployment capsule, and a container image. The
workspace depends on no crate outside `crates/`; OpenSSL is the only
third-party library, and ALSA (on the appliance), the C library, the Linux
uapi, and Win32 are the platform, all reached through hand-written
declarations in `crates/omt-sys`.

| Crate | Owns |
|---|---|
| `crates/omt-sys` | Every foreign declaration: libc/POSIX, OpenSSL, ALSA, the DRM uapi, Win32, with thin safe wrappers |
| `crates/omt-common` | Bounded buffers, strict JSON and XML, NFC, file and process I/O, randomness, time |
| `crates/omt-crypto` | The OpenSSL EVP adapters the Web frontend and deployer share |
| `crates/omt-protocol` | OMT wire transport and shared target validation |
| `crates/omt-vmx` | Decode-only VMX1, worker pool, AArch64 assembly kernels (`crates/omt-vmx/asm`) |
| `crates/omt-receiver-core` | Format policy, video ceilings, status projection |
| `crates/omt-receiver` | Linux adapters: DRM/KMS, ALSA, discovery, D-Bus, the `omt-receiver` binary |
| `crates/omt-test-sender` | First-party OMT A/V test sender |
| `crates/omt-web` | HTTPS operator GUI (poll loop, OpenSSL TLS), templates, diagnostics |
| `crates/omt-deploy` | Deployment jobs, the first-party SSH and SFTP client, SD-card prep, workstation probes |
| `crates/omt-capsule` | The manifest-v3 capsule, compiled into the deployer by its build script |
| `crates/rpi-omt-deploy` | `rpi-omt-deploy`, human and JSON-lines surfaces |
| `crates/rpi-omt-deploy-gui` | `rpi-omt-deploy-gui`, the browser deployer for Linux and Windows: a loopback server and its embedded page |
| `crates/omt-fuzz` | libFuzzer targets and their corpora (nightly only) |

| Directory | Owns |
|---|---|
| `deploy/` | Dockerfile, container scripts, host installer, OpenRC services, manifest-v3 capsule |
| `scripts/` | Build, gate, deploy, and release entry points |
| `tests/` | The ABI probe (`tests/abi`), shell, Python, native, and integration suites, schema vectors, test vectors |
| `tools/gen/` | Generators for committed sources: NFC tables, templates, Blowfish tables, the VMX entropy lookahead |
| `tools/toolbox/` | The image every gate runs inside |

Unsafe code is confined to `omt-sys`, `omt-vmx`, and `omt-fuzz`; every other
crate forbids it at its root.

Both deployer frontends run the same jobs from `crates/omt-deploy/src/jobs.rs`,
so a deployment means one thing regardless of which one the operator ran. The
deployer ships as a CLI and a browser frontend on both Linux and Windows. The
browser frontend is a loopback HTTP server with its page compiled in and the
appliance Web GUI's stylesheet served unchanged; it opens no graphics stack of
its own, so each is one self-contained file, and over SSH a forwarded port
reaches it.

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
make build-deployer           # Linux CLI + web GUI, static-PIE musl
make build-windows-deployer   # Windows CLI + web GUI, mingw-w64 with the pinned OpenSSL
make build-omt-sender         # OMT A/V test sender

# Verify
make test-web | test-receiver | test-deployer    # narrow suites (sanitized)
make test-sanitized | fuzz-smoke   # every Rust suite under ASan; every fuzz target
make test-quick               # every unit suite, no container engine (~1m)
make test                     # + Windows cross build, amd64 image, ARM64 builder stage
make lint                     # Rust static gates, ABI probe, supply chain, shell, docker, yaml, python, legal
make security-scan            # Trivy filesystem + image

# Run and ship
make up | down | logs         # local amd64 dev container
make deploy HOST=user@<ip>
make release                  # local pipeline; needs an authenticated gh CLI
```

Both deployer builds embed the appliance image, so `make build-arm64` comes
first; they stop and say so when `omt-client-arm64.tar.gz` is absent. It is
deliberately not a Make prerequisite, because an ARM64 image build runs the
appliance's runtime stage under emulation and should never start as a side
effect. Its Rust is cross-compiled on the workstation, so the build itself
takes a few minutes; the full pre-commit gate takes longer than that.

## Invariants

These are enforced by gates, not by convention — a change that breaks one fails
a commit rather than shipping.

- **Bounded by default.** Every read, subprocess, allocation, retry, and rate
  limit carries an explicit ceiling. `clippy.toml` refuses the unbounded std
  calls (`fs::read_to_string`, `Read::read_to_end`, unbounded line reads,
  `thread::spawn` without a stack size); `omt-common`'s bounded readers and
  checked cursors replace them. Unbounded input handling is a defect here even
  where growth looks impossible.
- **Lints are errors.** Clippy's `all` and `pedantic` groups are denied
  workspace-wide, with `unwrap`, `expect`, and `panic` refused outside tests,
  and every `unsafe` block carries a `SAFETY` comment and one unsafe operation.
  Release builds keep overflow checks on (except the decoder's kernels).
  `scripts/check-rust.sh` runs rustfmt and clippy for the host, the aarch64
  appliance, and the Windows deployer, and checks every generated source.
- **Unsafe is confined, and the foreign declarations are checked.** Only
  `omt-sys`, `omt-vmx`, and `omt-fuzz` may contain `unsafe`.
  `tests/abi/check_abi.py` compiles a probe of every hand-written struct
  layout and constant against the real C headers for each shipped target and
  fails on any difference.
- **Every suite runs sanitized.** `scripts/rust-sanitized.sh` runs the suites
  under AddressSanitizer with leak detection on a pinned nightly; every parser
  that reads untrusted input has a libFuzzer target in `crates/omt-fuzz`, and
  `make fuzz-smoke` runs them all. The AArch64 assembly kernels in
  `crates/omt-vmx/asm` are the one exception to the sanitizers; they are the
  only non-Rust source `scripts/check-no-c-sources.sh` allows, and the
  AArch64 suites (`scripts/test-arm64.sh`) hold them bit-exact to the
  portable Rust.
- **No third-party code but OpenSSL.** `scripts/check-rust.sh` fails if
  `Cargo.lock` names any package outside the workspace.
  `scripts/check-supply-chain.sh` allowlists every native library a crate may
  link, pins the Windows OpenSSL by version and SHA-256 to the 3.5 LTS series,
  and requires every base image to be pinned by digest.
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
