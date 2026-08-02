# Testing

Bootstrap the system dependencies, persistent ARM64 emulation, pinned Python
tools, the repository-local .NET SDK, and full-system Raspberry Pi OS VM
tooling once:

```bash
make install
```

Use the narrowest relevant gate:

```bash
make test-py
make test-receiver
make test-deployer
make test-quick
make test
./scripts/test-local.sh --full
```

`make test-py` covers validation, atomic state, persistent auth, Flask routes
against both the preview fakes and the real `ServiceContainer`, rate limits on
every throttled endpoint — including that an unparseable limit string fails
startup rather than silently serving unthrottled — About/System workflows,
reboot request correlation, target-correlated playback status, single-flight
source discovery, and runtime adapters, at a 98% branch-coverage floor. The
suite imports the
package straight from `src/` via the `pythonpath` setting in `pyproject.toml`,
so no install step is required.
`make test-receiver` performs locked restore, an analyzer-enabled core build, a
compile of the production receiver composition, shared validation vectors,
event-ordering and heartbeat-wait tests, atomic status-publication tests, HDMI
connector selection against a synthetic DRM sysfs tree, and a 95% receiver-core
branch gate.
`make test-deployer` performs locked restore, formatting/analyzers, unit and
headless Avalonia tests, and 95% coverage. Shell tests exercise entrypoint,
controller, deployment transactions, install/uninstall contracts, host helpers,
HDMI boot configuration, Compose, and supply-chain pins.

Timing-sensitive suites use `conftest.VirtualClock` rather than wall-clock
sleeps, so a budget assertion measures the code's own deadline arithmetic
instead of the host's filesystem latency.

`tests/unit/test_control_omt.sh` deliberately spends real seconds: it runs a
receiver that stays alive, which is the only way to reach the PID record, the
process-identity check that guards every kill, the lock the controller must not
leak into what it launches, and the SIGKILL fallback. Every controller
invocation there is wrapped in `timeout`, so a leaked lock names itself instead
of hanging the gate.

`scripts/test-local.sh` is the single entry point for every shell suite, and
`tests/unit/test_test_runner_args.sh` fails if a file in `tests/unit/` is not
wired into it.

The normal `make test` adds an amd64 image build. Full mode adds container smoke
and OMT receiver discovery/probe checks and requires the ARM64 receiver builder
stage to pass.

## Full Raspberry Pi OS VM

The optional full-system tier boots the checksum-pinned official Raspberry Pi
OS Full 64-bit image used by the hardware deployment, including its desktop and
recommended applications. It supplies an emulated ARM CPU, Raspberry Pi board,
SD card, USB Ethernet, and PID 1/systemd. Unlike the existing Pi-userland
container test, this reaches the real host installer, `/boot/firmware`, apt and
Docker service, systemd unit installation/verification, path-triggered host
diagnostics, the reboot validator, ARM64 image loading, and persistent Docker
state.
Image releases and checksums come from the
[official Raspberry Pi OS downloads](https://www.raspberrypi.com/software/operating-systems/).

On an x86-64 Linux host, run `make install`. It installs QEMU 9 or newer and
libguestfs tools natively when the distribution provides them. The required
commands are `qemu-system-aarch64`, `guestfish`, and `virt-resize`.

Debian or Ubuntu:

```bash
sudo apt-get update
sudo apt-get install -y qemu-system-arm libguestfs-tools openssh-client curl xz-utils
```

Fedora 43 or newer:

```bash
sudo dnf install -y --setopt=install_weak_deps=False \
    qemu-system-aarch64-core libguestfs guestfs-tools \
    openssh-clients curl xz kernel-core
```

RHEL 10 and compatible EL10 distributions do not provide the cross-
architecture `qemu-system-aarch64` binary in their standard repositories.
There, `make install` automatically builds a digest-pinned Fedora 44 tooling
image containing QEMU and libguestfs. The normal Make targets transparently run
inside a project-scoped Podman container with host networking and only this
checkout mounted. The container stays alive between `start`, `test`, `shell`,
and `stop`, while the VM disk remains under `.build/pi-os-vm` on the host. No
Fedora RPM is installed into EL10.

The host packages installed by `make install` on RHEL, Rocky Linux, AlmaLinux,
and CentOS can also be installed explicitly when preparing a development VM:

```bash
sudo dnf install -y \
    curl openssh-clients podman python3 ShellCheck tar xz
make install
```

`make install` then installs Hadolint when needed, configures persistent ARM64
user-mode emulation, and builds and verifies the full-system VM toolbox.

After `make install`, the commands are identical on every supported x86-64
Linux development host:

```bash
make build-arm64
make pi-os-vm-prepare  # one-time ~2 GiB download and persistent 16 GiB disk
make pi-os-vm-start
make test-vm
make pi-os-vm-debug    # optional bounded report under .build/pi-os-vm/
make pi-os-vm-shell    # optional interactive inspection
make pi-os-vm-stop
```

`pi-os-vm-prepare` downloads only the full-image URL in
`tests/vm/pi-os-image.env`, verifies its pinned SHA-256 before decompression,
expands a private copy of the root filesystem, and injects a one-time SSH key.
Allow roughly 24 GiB free during preparation; after the recoverable expanded
source is removed, the archive and persistent VM disk consume roughly 12 GiB.
The disk carries image name, checksum, and size metadata. A disk from the older
Lite-image harness or a later image revision is refused instead of being reused
silently.
SSH password authentication is disabled, forwarded SSH/Web ports bind only to
host loopback, and `test-vm` uploads only files named by
`deploy/manifest-v2.txt`. The persistent disk makes repeated installer and
upgrade investigations much faster. Change `PI_OS_VM_STATE_DIR`,
`PI_OS_VM_SSH_PORT_OVERRIDE`, and `PI_OS_VM_WEB_PORT_OVERRIDE` to isolate
concurrent instances.

The isolated tool definition is `tests/vm/Containerfile`; its Fedora base is
pinned in `tests/vm/tooling.env`. If a native QEMU/libguestfs installation is
missing or QEMU is older than version 9, `scripts/pi-os-vm.sh` delegates to this
tooling automatically. `make install` verifies that the resulting image has
all three required commands and supports the `raspi3b` machine before it
reports success.

The guest attempts to load `vkms` and `snd-dummy`. When the Raspberry Pi OS
kernel provides them, the suite can cross the systemd-to-Docker startup
boundary with virtual DRM and ALSA character devices. When it does not, the
test instead asserts the installer's documented startup deferral. It also
asserts that the installer converts the full desktop image from graphical boot
to its production `multi-user.target` and stops the display manager. Either way,
the VM is not a Raspberry Pi 5 hardware substitute: QEMU has no Pi 5 model, and
its Raspberry Pi 4 model lacks the GENET Ethernet controller needed by this
automated network test, as listed in the
[QEMU Raspberry Pi board documentation](https://www.qemu.org/docs/master/system/arm/raspi.html).
The harness therefore uses QEMU's stable `raspi3b`
model with USB Ethernet; the official ARM64 image and host userspace are the
same installation boundary, but the SoC is emulated as a Pi 3.

Pi-only validation still must cover the RP1/device topology, real vc4 DRM/ALSA,
HDMI hotplug and EDID, 1080p60 media timing, audio degradation, and a Web-
acknowledged successful reboot. The VM safely tests a correlated rejected
reboot request so the test runner does not disappear halfway through its gate.

The multi-stage container uses Microsoft's Alpine NativeAOT SDK only while
building the receiver. The .NET 10 ILC process stays on amd64 and
cross-compiles against an ARM64 Alpine sysroot. Under emulation, running the
compiler itself on ARM64 produced both a parallel-scanner access violation and
a single-threaded signature-parser failure; native Raspberry Pi compilation
was also reported to fail. The supported build path is therefore the x86-64
development or release VM, while the Pi consumes the resulting image as the
deployment/runtime target. After publish, NuGet packages and `bin`/`obj` trees
are removed, and a `scratch` artifact stage exports only `omt-receiver` and
`libvmx.so`. The integration gate caps that artifact image at 64 MiB and the
deployable Alpine runtime at 128 MiB; the SDK/compiler stage is never shipped
to the Pi.

On a systemd-based Linux x86-64 development VM, `make install` installs Podman,
configures full-system VM tooling, and runs `make setup-arm64-emulation`. The
setup extracts `qemu-aarch64` from a
digest-pinned `tonistiigi/binfmt` image, verifies the extracted binary hash,
installs it as `/usr/local/bin/qemu-aarch64-static`, and installs a
`systemd-binfmt` rule under `/etc/binfmt.d`. This is a host-level setup and
therefore requires sudo, a running systemd instance with `binfmt_misc`, and a
working Podman or Docker engine. The rule is restored on every boot and works
with rootless containers on SELinux hosts. Run `make setup-arm64-emulation`
again to repair or verify the registration. Docker Desktop users rely on its
Linux VM and should keep that engine running instead.

## Shared cross-language vectors

`tests/schema/` holds the contracts that the Python and C# suites both assert
against, so a change on one side fails the other:

| File | Contract |
|------|----------|
| `omt-target-vectors.json` | Source-name and direct-target validation |
| `playback-status-vectors.json` | Playback status fields, state enums, and projections |

`PlaybackStatusRecord.parse` (in `src/omt_client/playback_status.py`) requires
the field set to match exactly, so adding or renaming a status field in only one
language would make the receiver's output unparseable and pin the dashboard to
"Playback status stale". Update the vector file and both suites together.

The receiver publishes an unchanged projection at most once per
`StatusPublishPolicy.DefaultHeartbeat`, so that heartbeat must stay well under
the smallest accepted `OMT_PLAYBACK_STATUS_STALE_SECONDS`. The receiver suite
asserts that relationship directly.

## Lint gates

`./scripts/lint.sh` runs Bash syntax, ShellCheck, Hadolint, yamllint,
`ruff check`, `ruff format --check`, strict mypy over `src/` and `scripts/`, and
a relaxed mypy pass over `tests/`.

The legal gate is:

```bash
python3 scripts/check-legal-notices.py
```

It verifies that locked shipped Python/NuGet dependencies are represented in
the notices and that the legal files, About surfaces, OMT provenance, Docker
SBOM hook, and deployment capsule do not drift.
