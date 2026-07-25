# Testing

Bootstrap pinned Python and .NET tooling once:

```bash
make test-setup
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
every throttled endpoint, About/System workflows, reboot request correlation,
and runtime adapters, at a 98% branch-coverage floor. The suite imports the
package straight from `src/` via the `pythonpath` setting in `pyproject.toml`,
so no install step is required.
`make test-receiver` performs locked restore, analyzer-enabled build, shared
validation vectors, event-ordering tests, HDMI connector selection against a
synthetic DRM sysfs tree, and a 95% receiver-core branch gate.
`make test-deployer` performs locked restore, formatting/analyzers, unit and
headless Avalonia tests, and 95% coverage. Shell tests exercise entrypoint,
controller, deployment transactions, install/uninstall contracts, host helpers,
HDMI boot configuration, Compose, and supply-chain pins.

Timing-sensitive suites use `conftest.VirtualClock` rather than wall-clock
sleeps, so a budget assertion measures the code's own deadline arithmetic
instead of the host's filesystem latency.

`scripts/test-local.sh` is the single entry point for every shell suite, and
`tests/unit/test_test_runner_args.sh` fails if a file in `tests/unit/` is not
wired into it.

The normal `make test` adds an amd64 image build. Full mode adds container smoke
and OMT receiver discovery/probe checks. Pi-only validation still must cover
real DRM/ALSA, HDMI hotplug, 1080p60 media, audio degradation, service boot,
and an acknowledged Web reboot. The image-build integration test also checks
the ARM64 receiver artifacts when ARM64 emulation is registered; set
`REQUIRE_ARM64_BUILD=1` to make missing emulation a failure.

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
