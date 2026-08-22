# Testing

Build the gate toolbox once:

```bash
make install
```

Docker or Podman is the only thing the gates need from a workstation. Rust
1.97.1 with rustfmt and Clippy, the Windows GNU and musl targets, cargo-deny,
cargo-vet, Hadolint, ShellCheck, Trivy, mingw-w64, and the Python tooling all
live inside `tools/toolbox/Dockerfile`; nothing is installed onto the host.
`scripts/toolbox.sh` runs each gate in that image and rebuilds it automatically
when a pinned version changes, because the image tag is a content hash of the
Dockerfile, the Python requirements, and the pinned installers.

The toolbox is built on the same digest-pinned `rust:1.97.1-alpine3.23` image
the appliance compiles with, so the gates and the shipped receiver resolve one
compiler rather than two that can drift. Its musl host is also what makes the
static Linux deployer a native build rather than a cross-compile.

The repository is bind-mounted at its own absolute path rather than at a fixed
`/work`. Gates that start their own containers pass paths through the mounted
socket to the *host* daemon, which resolves them in its own filesystem, so any
other mount point would silently mount the wrong directory.

`scripts/install-dev-deps.sh` still provisions that toolchain on a host for
anyone who wants it, but no gate requires it any more.

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

Rebuild the toolbox with `make install` instead of narrowing the suite.

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
manifest-v3 path safety. It also holds the embedded capsule to the manifest it
ships: that the compiled-in member list is exactly `deploy/manifest-v3.txt`,
that every embedded name passes the real `valid_manifest_name` rule rather than
the copy of it in `build.rs`, and that the appliance archive is present, over a
mebibyte, and gzip. Because the capsule is a build input, this suite needs
`omt-client-arm64.tar.gz` in the project root; `make build-arm64` produces it
and both deployer build scripts stop with that instruction when it is absent. The egui application is built with its `desktop`
feature for the test run as well as the publish build, so the view is compiled
and linted rather than skipped, and the rules that enable its buttons are
tested against the same core validators the buttons' actions use. The same run
covers the rules that answer the display: that a window never opens larger than
the monitor it landed on and never grows to reach a floor, that an unreadable
monitor size changes nothing, that the opening fit is not spent after the
window has moved or after a short wall-clock launch budget, that a resize is
retried until the window is observed to fit, that the window is not centred
on the primary monitor, that the form column stops
widening, that labels stack at the narrowest window, and that zoom steps by a
tenth, saturates at its bounds, and returns to exactly 100%. None of that
needs a display attached.

The deployer core tests also pin the initial SD-card configuration to LF line
endings, hex SSIDs, derived PSKs, uppercase regulatory countries, and an Alpine
boot-partition marker. They do not download from the network. For a release
candidate, run `prepare-sd` against a freshly imaged Alpine card, confirm the
two files and pinned overlay digest on the mounted partition, then boot the Pi
and complete the Alpine view over its headless SSH service.

`tests/native/test_deployer_cli.sh` then runs the built CLI: capsule
validation with and without a project root -- including that an embedded
deployment reports no workstation prerequisite at all and that `--rebuild-image`
is refused without a tree to build from -- the exit-2 usage contract for missing
arguments and rejected connections, the bounded `--secrets-stdin` channel, Alpine sys-setup argument
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
application for `x86_64-pc-windows-gnu` and publishes the package. Every
non-quick local run performs the same cross build with `--no-publish`, which
compiles and header-verifies the executables without staging a package, so a
broken cross build stops a commit while both published packages still come off
one commit's sources.

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
and the Trivy scans. No test mode publishes an artifact.

## Local hooks

`./scripts/setup-hooks.sh` — run by `make install` — points `core.hooksPath` at
the tracked `.githooks/` directory, so both hooks update with a checkout
instead of drifting as copies under `.git/hooks`.

| Hook | Runs |
|------|------|
| `.githooks/pre-commit` | `./scripts/test-local.sh --full`, `scripts/audit-python-deps.sh`, `scripts/security-scan.sh` |
| `.githooks/post-commit` | `make build-arm64`, `make build-deployer`, `make build-windows-deployer` |

The publishing builds run after the commit rather than before it because every
shipped artifact bakes in the version `scripts/detect-version.sh` reports —
the receiver, the web service, both deployer executables, and the SBOMs beside
them. Publishing from the pre-commit gate stamps them against a tree that is
not yet a commit; publishing afterwards means an artifact's version is the one
its commit carries. The ARM64 image is built first because both deployer
executables compile it into themselves.

Git ignores a post-commit hook's exit status, so a failed build cannot undo the
commit; the hook prints a `FAILED` banner and the `make` target to rerun. It
skips itself while a rebase, cherry-pick, revert, or merge sequencer is
replaying commits — publishing on each intermediate commit would rebuild the
image repeatedly and leave the packages describing whichever commit landed
last. Publish from the finished sequence with
`make build-arm64 build-deployer build-windows-deployer`. `git commit
--no-verify` bypasses the pre-commit gate only; the post-commit builds still
run.

## Publishing a GitHub Release locally

After the final versioned commit is on a clean branch with an upstream, run:

```bash
gh auth login
make release
```

This does not use GitHub Actions. It reruns the ARM64 image and Linux/Windows
deployer publishers locally, packages the two deployer directories with fixed
timestamps and ownership, writes `SHA256SUMS`, creates the annotated tag named
by `workspace.package.version`, and atomically pushes the branch and tag before
calling the GitHub Release API. `0.x` versions are marked as prereleases and
release notes are generated from the repository history.

The command refuses a dirty or detached worktree, a branch without an
upstream, and a version tag that already belongs to another commit. It never
replaces assets on an existing GitHub Release. The staged upload files remain
under `.build/github-release/<version>/` for inspection or recovery.

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
**each supported board** — Pi 5 and Pi 4 Model B. The boards differ in RAM and
decode throughput, so a pass on one is not evidence for another:

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
   must be refused, since its model string starts with a supported prefix, and
   so must a Pi 3 or Zero 2 W, which are refused for having no 5 GHz radio;
2. reboot, then verify all four `omt-client*` OpenRC services and nftables;
3. verify HDMI connectors, hotplug, EDID, video at the board's ceiling, and
   HDMI audio. Verify both connectors on each board. Include the multi-card
   fallback: the unit suite drives it against a fake sysfs tree, but selecting
   the second physical connector after the first card's attributes fail to read
   is not reproducible off the board;
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
   the decoder three of the four cores. Both shipped ceilings in
   `deploy/lib/board-profile.sh` are measured; if a board stops sustaining the
   frame interval its ceiling promises, lower that board's profile rather than
   shipping a limit it cannot hold. Then confirm that over-ceiling input reports
   `unsupported-format` rather than stuttering;
7. verify the resampled path on a display whose mode list does not carry the
   sender's format — a panel that stops at 720p fed 1080p is the case this was
   written for. Confirm a full picture rather than `unsupported-format`, that
   the running detail names both sizes, that a mismatched aspect ratio gets
   black bars rather than a stretch, and that the board still holds the frame
   interval with the resample in the loop. This path is unit-tested only; it is
   on the DRM hardware boundary and has not been exercised on a Pi;
8. verify HDMI audio timing under a loaded link. Play a full session over the
   Wi-Fi link the appliance will actually use and confirm the playing detail
   reports no underruns, then confirm the bundle's ALSA playback stream state
   shows the negotiated buffer and a start threshold near 100 ms rather than
   ALSA's one-frame default. The start threshold and ring size are set through
   the device and cannot be asserted off the board;
9. induce a video TCP disconnect on a Pi 4 and a Pi 5 while a session is
   playing. The two ways it can fail are not interchangeable, and each has its
   own bound, so exercise both:

   - **the socket closes.** Restart the sender, or reset the video connection
     so this end observes it. The last picture and the audio must both survive
     the in-session reconnect with no gap in sound and no black frame, and the
     playing detail must then name a video reconnect. The window for that is
     narrow, and knowing its size is the point of this step: a refused
     connection fails as fast as the kernel answers, so the three attempts are
     spent in the backoffs alone — measured at 831 ms on a Pi 4 — and only a
     port that swallows the SYN takes the full four seconds. So a sender
     restarted within roughly 350 ms is reconnected in-session, and one
     restarted after a second is not: it must read `in-session video reconnects
     did not hold`, re-resolve discovery, and rebuild DRM and audio together.
     Both outcomes are correct. A *slow* restart landing in the first is the
     regression to watch for, because it means the budget was widened, and the
     whole cost of a wider budget is that every dead endpoint now holds a
     frozen picture for longer. Restore the port and confirm the count resets —
     a link that drops occasionally must never exhaust the budget;
   - **the socket stays open and goes quiet.** An `nft` rule on the video port
     does *not* close anything: the connection stays ESTABLISHED, every read
     returns `WouldBlock`, and the reconnect budget is never armed, because it
     is armed only by a channel that reports itself disconnected. This is the
     shape a firewall, a NAT timeout, or an access point that forgets the
     association produces. The detail must read `Waiting for video frames.`
     while audio keeps playing, and then, within `MEDIA_STALL`, the session
     must fail with `No video frames for 15 seconds on a connected socket.`
     and rebuild. A session that sits in `Waiting for video frames.`
     indefinitely is the regression this step exists to catch;
10. confirm a held frame on damaged input. Nothing off the board proves that a
    skipped frame leaves the previous picture scanning out: the unit tests
    cover only which decoder faults are allowed to skip and how long a run is
    tolerated, and the hold itself is DRM behaviour. Feed a sender emitting
    corrupt VMX bodies, or corrupt them in flight, and confirm the picture
    freezes rather than tearing or going black, that audio continues, that the
    playing detail counts the skips, and that a sustained run ends the session
    with `VMX decoder rejected 21 consecutive frames` rather than freezing
    indefinitely on `running`.

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
