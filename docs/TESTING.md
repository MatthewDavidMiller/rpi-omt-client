# Testing

Bootstrap the system dependencies, persistent ARM64 emulation, pinned Python
tools, and the repository-local .NET SDK once:

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
stage to pass. Pi-only validation still must cover
real DRM/ALSA, HDMI hotplug, 1080p60 media, audio degradation, service boot,
and an acknowledged Web reboot.

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

On a systemd-based Linux x86-64 development VM, `make install` installs Podman
and runs `make setup-arm64-emulation`. The setup extracts `qemu-aarch64` from a
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
