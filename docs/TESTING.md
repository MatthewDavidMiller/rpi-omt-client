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

`make test-py` covers validation, atomic state, persistent auth, Flask routes,
About/System workflows, reboot request correlation, and runtime adapters.
`make test-receiver` performs locked restore, analyzer-enabled build, shared
validation vectors, event-ordering tests, and a 95% receiver-core branch gate.
`make test-deployer` performs locked restore, formatting/analyzers, unit and
headless Avalonia tests, and 95% coverage. Shell tests exercise entrypoint,
controller, deployment transactions, install/uninstall contracts, host helpers,
Compose, and supply-chain pins.

The normal `make test` adds an amd64 image build. Full mode adds container smoke
and OMT receiver discovery/probe checks. Pi-only validation still must cover
real DRM/ALSA, HDMI hotplug, 1080p60 media, audio degradation, service boot,
and an acknowledged Web reboot. The image-build integration test also checks
the ARM64 receiver artifacts when ARM64 emulation is registered; set
`REQUIRE_ARM64_BUILD=1` to make missing emulation a failure.

The legal gate is:

```bash
python3 scripts/check-legal-notices.py
```

It verifies that locked shipped Python/NuGet dependencies are represented in
the notices and that the legal files, About surfaces, OMT provenance, Docker
SBOM hook, and deployment capsule do not drift.
