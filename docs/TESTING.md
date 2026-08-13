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
make test-web
make test-receiver
make test-deployer
make build-windows-deployer
make test-quick
make test
./scripts/test-local.sh --full
```

For full network, codec, audio, and real-Pi playback validation, build the
first-party Rust sender with `make build-omt-sender`. Its source-scoped firewall
setup, lifecycle commands, ARM64 build, and end-to-end checklist are documented
in [OMT_TEST_SENDER.md](OMT_TEST_SENDER.md). The normal Rust gate compiles and
tests it; the shell gate also asserts that its manifest adds no third-party
package dependency. Before calling a sender build Pi-compatible, build
`aarch64-unknown-linux-musl` and run it on a real Alpine aarch64 Pi. Pi 4 and Pi
5 share that userspace ABI, while receiver throughput and HDMI behavior still
require a display-path check on the board being qualified.

`make test-web` builds and tests the Rust HTTPS service. It covers target and
network validation, state, legacy and current password hashes, persistent
authentication, secure cookies, CSRF/rate limits, security headers, every
authenticated page, diagnostics, and runtime adapters.

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
caps. The worker-pool unit suite also forces a channel failure after another
worker has started, proving that the pool drains every outstanding raw-pointer
job before returning the error.

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
connections, the bounded `--secrets-stdin` channel, Alpine sys-setup argument
checks, and the one-object-per-line
`--json` surface. Nothing in it reaches the network -- every invocation is
local or refused before a connection is opened. The SSH adapter rejects missing
default or explicitly selected `known_hosts` files and unknown or changed host
keys; legacy SHA-1 host-key hashes and CBC ciphers are excluded from
negotiation. An empty SSH password is valid for factory Alpine `root` and tries
`none`, password, and keyboard-interactive auth. Privileged command construction is tested for password-backed
sudo, passwordless sudo, direct root sessions, and the separate root-secret
gate used to bootstrap untouched Alpine through `su`.
That gate also covers stock Alpine's misleading password-capable `doas` probe;
an explicit initial root credential must select the PTY-backed `su` path.
The SSH adapter's command timeout is long enough for a clean-host package
installation while retaining its shorter idle timeout and bounded output.
The deployer summary parser drops package-manager noise and retains the final
installer URL/status block after successful native deployments.

The reboot bridge tests include the install-time empty result channel as well
as populated files, unsafe modes, and symlinks. This protects the fixed-inode
publication boundary without depending on the human-readable file description
returned by a particular `stat` implementation.

The supply-chain gate rejects every tracked C/C++ source, Git Cargo dependency,
or unlocked registry package. `scripts/check-supply-chain.sh` also runs
`cargo deny` and `cargo vet` against `deny.toml` and `supply-chain/`. Container
integration checks that neither Python nor a C++ standard-library payload ships.

The ARM64 artifact contract also pins the receiver-source fingerprint at both
of Podman's cross-stage COPY boundaries. A receiver change must alter the
published runtime image instead of merely recompiling an unused builder layer.
The same contract refuses a broad `COPY crates/ crates/`, which would make
unrelated deployer edits invalidate the slow ARM64 receiver build.

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

The live nftables reachability case uses a private user, mount, and network
namespace when the workstation permits unprivileged user namespaces. Its
mapped root has network administration rights only over that throwaway stack.
On a workstation where user namespaces are disabled, the same case requires
passwordless sudo; it never replaces the packet test with a text-only check.

The normal `make test` adds the Windows cross build, an amd64 image build, and
the ARM64 receiver builder stage. Full mode adds container smoke and OMT
network tests. The pre-commit hook runs full mode, the Python dependency audit,
and the Trivy scans.

## Hardware validation boundary

There is intentionally no full-system Raspberry Pi VM tier. QEMU models none of
the supported SoCs, so a guest cannot validate RP1, any board's device tree, vc4
KMS/HDMI audio, device groups, or the supported board preflight. The previous
Raspberry Pi OS VM was also the wrong host OS and has been removed.

"Clean Alpine" means genuinely untouched, and that is the state most likely to
break a deployment: the image has no `bash`, no `sudo`, no `community`
repository, no `/dev/dri`, and no `/dev/snd`. Validating against a host that was
prepared by hand hides exactly the bugs this tier exists to catch, so reflash
or reset the card rather than reusing a previously deployed one.

Before release, validate on a clean Alpine 3.24 aarch64 `sys` installation on
**each supported board** — Pi 5, Pi 4 Model B, Pi 3, and Zero 2 W. The boards
differ in HDMI count, ALSA card layout, RAM, and decode throughput, so a pass on
one is not evidence for another:

0. on a factory diskless image, confirm the Alpine view / `alpine-setup`
   command runs `deploy/host/setup-sys.sh`, installs persistent sys mode, and
   that `deploy/host/bootstrap.sh` then installs bash and sudo before the
   installer runs. A typical factory/headless image has no SFTP subsystem and
   a 1970 clock; the deployer must still upload the script and HTTPS apk
   mirrors must verify.
   Confirm the memory cgroup is live after the reboot (`grep -qw memory
   /sys/fs/cgroup/cgroup.controllers` on the shipped cgroup-v2 host, then check
   that the container's `memory.max` is `134217728`): the Pi firmware injects
   `cgroup_disable=memory`, and without the installer's
   `cgroup_enable=memory` the advertised container memory cap is silently not
   enforced. `/proc/cgroups` is only the fallback check for a cgroup-v1 host;

1. deploy through both CLI and native app and confirm unsupported boards fail
   before upload. Include a near miss if one is available: a Pi 400 or Pi 500
   must be refused, since its model string starts with a supported prefix;
2. reboot, then verify all four `omt-client*` OpenRC services and nftables;
3. verify HDMI connectors, hotplug, EDID, video at the board's ceiling, and
   HDMI audio. On the Pi 4 and Pi 5 verify both connectors; on the Pi 3 and
   Zero 2 W verify that the single output works and that its unindexed
   `vc4hdmi` ALSA card is the one opened. Include the multi-card fallback: the
   unit suite drives it against a fake sysfs tree, but selecting the second
   physical connector after the first card's attributes fail to read is not
   reproducible off the board;
4. verify zram, the 128 MiB memory limit, 64 PID limit, bounded Docker logs,
   and stable operation under memory pressure;
5. verify discovery/direct playback, support bundle correlation/PCAP opt-in,
   Wi-Fi mutation, and a Web-acknowledged reboot;
6. confirm the board's decode ceiling with

   ```bash
   cargo test --release -p vmx-decoder --test decode_bench -- --ignored --nocapture
   ```

   A cross-built standalone benchmark can read staged vectors from an explicit
   directory with `VMX_VECTOR_DIR=/path/to/vectors`; otherwise it uses the
   repository's `tests/vectors/vmx` directory.

   The 3-worker row is the one that decides a tier, because the receiver gives
   the decoder three of the four cores. The shipped ceilings in
   `deploy/lib/board-profile.sh` are derived from core count and clock rather
   than measured; if a board cannot sustain the frame interval its ceiling
   promises, lower that board's profile rather than shipping a limit it cannot
   hold. The Zero 2 W's 720p60 is the most optimistic entry and 720p30 may be
   what it actually sustains. Then confirm that over-ceiling input reports
   `unsupported-format` rather than stuttering.

Useful commands are in `docs/OPERATIONS.md`. Any release not completing this
physical tier must state the skipped Pi-specific checks.

## Cross-file and cross-language contracts

`tests/unit/test_cross_file_invariants.py` asserts the constants one file
computes with and another supplies: the host diagnostics budget spelled out in
the Rust Web settings, `install.sh`, and `host-diagnostics.sh`, and the HDMI connector
names the container launcher, the receiver CLI, and the status contract must
all accept.

`tests/schema/omt-target-vectors.json` and
`tests/schema/playback-status-vectors.json` are consumed by the Rust tests.
Both binaries share `omt-protocol` target validation, while the receiver and
Web suites assert the status projection. The target vectors publish forbidden
source-name code points as ranges and the compiled table is asserted against
them. `tests/vectors/vmx/vectors.json` indexes the VMX conformance
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

It checks shipped Rust/Alpine dependencies, legal/About surfaces, OMT
provenance, SBOM hooks, and the deployment capsule.
