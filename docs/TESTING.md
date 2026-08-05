# Testing

Bootstrap Python tooling, the native C/C++ toolchain, the mingw-w64 Windows
cross toolchain, Hadolint, Trivy, and ARM64 container emulation once:

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

`make test-receiver` builds the receiver with Clang AddressSanitizer and
UndefinedBehaviorSanitizer. It exercises shared target vectors, bounded wire
parsing, CLI exit-status contracts, detail sanitization and JSON escaping,
playback state/order, heartbeat publication, and atomic status replacement.

`make test-deployer` builds the dependency-free C++ core with strict warnings
and tests validation, quoting, SHA-256, secure tokens, and manifest v3 path
safety. Full
mode also resolves hash-locked SDL3, Dear ImGui, and libssh2 archives and
publishes the host-native GUI.

Restricted or offline builders may point the publish gate and the Windows cross
build at trusted, previously verified source trees with
`RPI_OMT_SDL3_SOURCE_DIR`, `RPI_OMT_IMGUI_SOURCE_DIR`, and
`RPI_OMT_LIBSSH2_SOURCE_DIR`. Otherwise CMake downloads the pinned archives and
verifies their SHA-256 locks before use.

`make build-windows-deployer` cross-compiles the deployment application for
Windows x86-64 with mingw-w64 and verifies the PE headers it cannot execute:
64-bit GUI subsystem, ASLR, DEP, high-entropy address randomization, and no
redistributable runtime imports. Every non-quick local run performs this cross
build, so both published deployer packages come off the same commit.

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
native C/C++ tests.
Update both implementations and their vectors together when a shared contract
changes. Playback status heartbeat must remain well below the smallest accepted
staleness threshold.

## Lint and legal gates

`./scripts/lint.sh` runs Bash syntax, ShellCheck, Hadolint, yamllint, Ruff, and
mypy. The legal gate is:

```bash
python3 scripts/check-legal-notices.py
```

It checks shipped Python/native dependencies, legal/About surfaces, OMT
provenance, SBOM hooks, and the deployment capsule.
