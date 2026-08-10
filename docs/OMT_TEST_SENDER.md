# OMT Test Sender

The repository includes a first-party Rust OMT source for repeatable receiver
testing. It uses the same workspace protocol definitions as the receiver and
adds no third-party package dependency. It streams:

- reference-encoded VMX1 1920x1080 progressive video at 60 fps, alternating
  the committed gradient and flat conformance frames;
- stereo 48 kHz planar FPA1 audio with a quiet 480 Hz test tone; and
- separate, subscription-driven TCP video and audio connections, matching the
  receiver's production handshake.

The sender is developer test tooling. It is not copied into the appliance
image or deployment capsule, and it does not advertise through mDNS. Use the
printed direct `omt://` URI so the endpoint under test is unambiguous.

## Build and platform support

A normal native build needs only the repository's existing Rust toolchain:

```bash
make build-omt-sender
```

The build is locked to `Cargo.lock` and stages the executable under
`.build/omt-test-sender`. It fetches no OMT source tree, SDK image, codec, or
sample media: both VMX frames are compiled from `tests/vectors/vmx/`, and FPA1
audio is produced by Rust code.

For Alpine aarch64 on either a Pi 4 or Pi 5, build the static musl artifact:

```bash
make build-omt-sender OMT_SENDER_TARGET=aarch64-unknown-linux-musl
```

This is the same target, linker, and self-contained linking model used for the
receiver. `make install` provisions the Rust target. The result is:

```text
.build/omt-test-sender/artifacts/aarch64-unknown-linux-musl/bin/omt-test-sender
```

A cross-built executable is deliberately not selected by the workstation's
`current` symlink. Copy the single executable to the Pi and run it directly;
it requires no shared C/C++, .NET, codec, or media-file runtime dependency.
The supported Pi userspace is the project's 64-bit Alpine environment. A
32-bit Pi OS installation is not compatible with the aarch64 artifact.

## Firewall

The sender listens on the first available TCP port in 6400-6600. Allow that
range only from the receiver address or a narrow lab subnet:

```bash
make omt-sender-firewall-allow SOURCE=10.1.20.210
./scripts/configure-omt-test-sender-firewall.sh status 10.1.20.210
```

The firewalld helper applies both runtime and persistent source-scoped TCP
rules. A single receiver address is preferred; CIDRs broader than `/16`, IPv6,
malformed input, and unsafe zone names are refused. Set
`OMT_TEST_SENDER_FIREWALL_ZONE` if the sender interface is not in `public`.
Root or non-interactive sudo is required.

Remove the rules after the test:

```bash
make omt-sender-firewall-remove SOURCE=10.1.20.210
```

Removal also cleans the UDP 5353 allowance created by an earlier sender design.
On a host without firewalld, create an equivalent source-scoped TCP 6400-6600
rule with the host firewall. Do not disable the firewall or expose the range to
an untrusted network.

## Run and connect

Start the native sender and use its printed endpoint:

```bash
make omt-sender-start
make omt-sender-status
# OMT test sender running: omt://192.0.2.10:6400 (pid ...)
```

The lifecycle wrapper keeps PID, kernel process-start identity, port, and a
startup-rotated log under `.build/omt-test-sender/runtime`. It refuses a second
managed instance, refuses a symlink log, and will not signal a process whose
identity no longer matches.

```bash
./scripts/omt-test-sender.sh logs
make omt-sender-stop
```

For a foreground process, useful on a Pi or under `timeout`, use the staged
binary directly or run `./scripts/omt-test-sender.sh run` on the native host.
The binary accepts `--bind IP` and `--port PORT`; without `--port`, it scans only
6400-6600.

## End-to-end checks

First verify the protocol path without requiring HDMI:

```bash
omt-receiver probe --target omt://SENDER:PORT --timeout-ms 5000 --json
```

The result should report VMX1 1920x1080 at 60 fps and FPA1 stereo at 48 kHz.
Then attach an HDMI display or capture sink and confirm playback reaches
`running`, the alternating gradient/flat picture is visible, and the tone is
present on HDMI. Exercise disconnect/reconnect and sender shutdown to check the
documented retry states.

The sender validates subscription, framing, VMX decode input, audio samples,
and network behavior. It cannot prove DRM scanout, page flips, EDID mode
selection, or HDMI audio when the Pi reports no connected display.

## Test coverage

`cargo test -p omt-test-sender` parses generated frames with `omt-protocol` and
checks CLI and timestamp behavior. `tests/unit/test_omt_test_sender.sh` gates
the dependency contract, ARM64 build path, lifecycle safety, and firewall
scope. `make test-receiver` also runs a real sender-to-receiver TCP probe before
cross-checking the ARM64 receiver decoder under emulation.
