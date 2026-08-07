# Testing

Bootstrap Python tooling, Rust 1.97.1 with rustfmt/Clippy and the Windows GNU
target, Hadolint, Trivy, and ARM64 container emulation once:

```bash
make install
```

`make install` fails if it cannot provision one of those tools, because every
gate below runs on every commit and none of them substitute a pass for a tool
they could not find.

## No gate skips

There is no mode, flag, or environment variable that makes a gate report a pass
for a check it did not run:

- a missing Hadolint, ShellCheck, yamllint, Ruff, mypy, Trivy, or Python 3
  fails its gate rather than stepping over it;
- unregistered ARM64 emulation fails the container gate rather than omitting
  the appliance's own architecture;
- a skipped, expected-failure, or deselected pytest case fails the Python run
  through `tests/conftest.py`;
- `tests/unit/test_supply_chain.sh` fails if a new skip escape appears in
  `tests/`, `scripts/`, or `tools/`.

Fix the workstation with `make install` instead of narrowing the suite.

Use the narrowest relevant gate:

```bash
make test-py
make test-receiver
make test-deployer
make build-windows-deployer
make test-quick
make test
./scripts/test-local.sh --full
```

`make test-py` covers validation, state, persistent auth, CSRF/rate limits,
routes, diagnostics, and runtime adapters at a 98% branch-coverage floor.

`make test-receiver` builds and tests the Rust receiver crates. It exercises
shared target vectors, bounded wire parsing, CLI exit-status contracts, detail
sanitization and JSON escaping, playback state/order, heartbeat publication,
and atomic status replacement. It drives a loopback discovery server for both
the single-source contract and the multi-source announcement stream, covering
withdrawal, re-announcement, sorting, and rejection of names or ports that fail
the shared grammars.

VMX decoding is checked against `tests/vectors/vmx/`, conformance streams
captured from the Open Media Transport reference decoder before its removal.
Every stream must decode to the exact bytes the reference produced, in both
UYVY and BGRX, at one, two, three, and eight workers, so a worker count can
never change an output byte. The same suite covers repeated decode lifecycles,
every truncation of a valid stream, and periodic bit flips through the payload,
none of which may panic, read out of bounds, or allocate past the documented
caps.

`make test-deployer` builds the Rust core, CLI, and GUI and tests validation,
quoting, SHA-256, secure tokens, Wi-Fi PSK vectors, bounded processes, and
manifest-v3 path safety. Publish mode builds the egui GUI and CLI. The SSH
adapter rejects missing `known_hosts` files and unknown or changed host keys;
legacy SHA-1 host-key hashes and CBC ciphers are excluded from negotiation.

The supply-chain gate rejects every tracked C/C++ source, Git Cargo dependency,
or unlocked registry package. `scripts/check-supply-chain.sh` also runs
`cargo deny` and `cargo vet` against `deny.toml` and `supply-chain/`. Container
integration checks that no C++ standard-library payload ships and that Python's
decimal fallback remains functional.

Restricted or offline builders use a trusted Cargo registry mirror populated
with the exact checksums in `Cargo.lock`.

`make build-windows-deployer` cross-compiles the deployment CLI and egui
application for `x86_64-pc-windows-gnu`. Every non-quick local run performs
this cross build, so both published packages come off the same commit.

`make test-quick` runs every shell contract/behavior suite, receiver tests,
deployer tests, lint, and Python tests without requiring a container image
build or the Windows cross build. Shell coverage includes the Alpine-only preflight, OpenRC definitions,
installer hardening/firmware contract, reboot validation, host diagnostics,
HDMI `usercfg.txt` rules, transactions, Compose resource limits, and supply
chain pins.

The normal `make test` adds the Windows cross build, an amd64 image build, and
the ARM64 receiver builder stage. Full mode adds container smoke and OMT
network tests. The pre-commit hook runs full mode, the Python dependency audit,
and the Trivy scans.

## Hardware validation boundary

There is intentionally no full-system Raspberry Pi VM tier. QEMU has no
Raspberry Pi 5 model, so a raspi3/raspi4 guest cannot validate RP1, the Pi 5
device tree, vc4 KMS/HDMI audio, device groups, or the supported board preflight.
The previous Raspberry Pi OS VM was also the wrong host OS and has been removed.

Before release, validate on a physical low-RAM Pi 5 running a clean Alpine 3.23
aarch64 `sys` installation:

1. deploy through both CLI and native app and confirm non-Alpine/non-Pi-5
   targets fail before upload;
2. reboot, then verify all four `omt-client*` OpenRC services and nftables;
3. verify both HDMI connectors, hotplug, EDID, 1080p60 video, and HDMI audio;
4. verify zram, the 256 MiB memory limit, 64 PID limit, bounded Docker logs,
   and stable operation under memory pressure;
5. verify discovery/direct playback, support bundle correlation/PCAP opt-in,
   Wi-Fi mutation, and a Web-acknowledged reboot.

Useful commands are in `docs/OPERATIONS.md`. Any release not completing this
physical tier must state the skipped Pi-specific checks.

## Shared cross-language vectors

`tests/schema/omt-target-vectors.json` and
`tests/schema/playback-status-vectors.json` are consumed by Python and the
Rust tests. `tests/vectors/vmx/vectors.json` indexes the VMX conformance
streams and pins each expected image by SHA-256, which keeps the fixtures small
while still asserting bit exactness.
Update both implementations and their vectors together when a shared contract
changes. Playback status heartbeat must remain well below the smallest accepted
staleness threshold.

## Lint and legal gates

`./scripts/lint.sh` runs rustfmt, Clippy, the supply-chain gate, Bash syntax,
ShellCheck, Hadolint, yamllint, Ruff, and mypy. The legal gate is:

```bash
python3 scripts/check-legal-notices.py
```

It checks shipped Python/native dependencies, legal/About surfaces, OMT
provenance, SBOM hooks, and the deployment capsule.
