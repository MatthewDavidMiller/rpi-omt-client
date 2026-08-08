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
routes, diagnostics, and runtime adapters at a **100%** branch-coverage floor.
The floor is 100 rather than a margin below it because a margin is only ever
spent on branches nobody chose to leave uncovered: the two it was hiding were
the receive probe's success path and a bundle collected against an unreadable
target, both of which an operator reads directly out of a support archive.

`make test-receiver` builds and tests the Rust receiver crates. It exercises
shared target vectors, bounded wire parsing, CLI exit-status contracts, detail
sanitization and JSON escaping, playback state/order, heartbeat publication,
and atomic status replacement. HDMI connector selection is driven against a
temporary directory standing in for `/sys/class/drm` and `/dev/dri`, covering
card ordering, a renumbered binding, and each way a card can be half-populated
without hiding a connected one behind it. It drives a loopback discovery server for both
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
manifest-v3 path safety. The egui application is built with its `desktop`
feature for the test run as well as the publish build, so the view is compiled
and linted rather than skipped, and the rules that enable its buttons are
tested against the same core validators the buttons' actions use. The same run
covers the rules that answer the display: that a window never opens larger than
the monitor it landed on and never grows to reach a floor, that an unreadable
monitor size changes nothing, that the form column stops widening, that labels
stack at the narrowest window, and that zoom steps by a tenth, saturates at its
bounds, and returns to exactly 100%. None of that needs a display attached.

`tests/native/test_deployer_cli.sh` then runs the built CLI: capsule
validation, the exit-2 usage contract for missing arguments and rejected
connections, the bounded `--secrets-stdin` channel, and the one-object-per-line
`--json` surface. Nothing in it reaches the network -- every invocation is
local or refused before a connection is opened. The SSH adapter rejects missing
`known_hosts` files and unknown or changed host keys; legacy SHA-1 host-key
hashes and CBC ciphers are excluded from negotiation.

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
3. verify both HDMI connectors, hotplug, EDID, 1080p60 video, and HDMI audio.
   Include the multi-card fallback: the unit suite drives it against a fake
   sysfs tree, but selecting the second physical connector after the first
   card's attributes fail to read is not reproducible off the board;
4. verify zram, the 256 MiB memory limit, 64 PID limit, bounded Docker logs,
   and stable operation under memory pressure;
5. verify discovery/direct playback, support bundle correlation/PCAP opt-in,
   Wi-Fi mutation, and a Web-acknowledged reboot.

Useful commands are in `docs/OPERATIONS.md`. Any release not completing this
physical tier must state the skipped Pi-specific checks.

## Cross-file and cross-language contracts

`tests/unit/test_cross_file_invariants.py` asserts the constants one file
computes with and another supplies: the Gunicorn worker timeout the diagnostics
bundle ceiling is derived from, the host diagnostics budget spelled out in
`settings.py`, `install.sh`, and `host-diagnostics.sh`, and the HDMI connector
names the container launcher, the receiver CLI, and the status contract must
all accept.

`tests/schema/omt-target-vectors.json` and
`tests/schema/playback-status-vectors.json` are consumed by Python and the
Rust tests. Both halves of the status contract are asserted against the shared
file, so neither a state only one side knows nor one no producer emits can
survive: Rust checks `video_states` is exactly what `video_name` produces, and
Python checks `PUBLIC_STATES` is total over `receiver_states`. The target vectors also publish the forbidden source-name code
points as ranges: the Python suite derives them from `unicodedata` and the
receiver's compiled table is asserted against the published one, so neither
validator can drift from the other or from a Unicode revision. `tests/vectors/vmx/vectors.json` indexes the VMX conformance
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
