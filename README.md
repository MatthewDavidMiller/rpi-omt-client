# Raspberry Pi OMT Client

Raspberry Pi OMT Client receives Open Media Transport (OMT) video and audio on
a Raspberry Pi 5 and presents it directly on HDMI. It combines a native
.NET 10 OMT receiver, direct DRM/KMS video output, ALSA audio, a hardened Flask
Web GUI, and a Windows deployment GUI.

The only supported appliance host is a Raspberry Pi 5 running Alpine Linux
3.23 aarch64 in persistent `sys` mode. Diskless Alpine and Raspberry Pi OS are
not supported.

The receiver supports discovered source names and explicit
`omt://host:port` targets. Video is limited to 1920×1080 at 60 fps; larger or
faster input is reported as unsupported instead of being silently converted.

## Build and test

```bash
make install
make test-quick
make build-arm64
make build-windows-deployer
```

The ARM64 build creates `omt-client-arm64.tar.gz`. The Windows build creates
`dist/rpi-omt-client-deployer-windows-x64.exe` and its CycloneDX SBOM.
On Linux x86-64, `make install` also installs persistent ARM64 emulation for
the container builder.

## Install

Copy the nested files named in `deploy/manifest-v2.txt` to the Pi, then run:

```bash
sudo ./deploy/host/install.sh
```

Or deploy over SSH:

```bash
make deploy HOST=admin@192.168.1.50
```

The installer verifies the OS and board, installs Alpine's Pi kernel, firmware,
DRM/ALSA tooling, Docker and OpenRC services, and applies appliance hardening
and low-memory defaults. It prints the authoritative HTTPS URL and initial
password instructions. An incompatible predecessor installation must be
uninstalled first; this release does not migrate predecessor state.

## Operator UI

The authenticated Web GUI provides:

- source selection, playback state, restart, and stop/clear;
- one optional OMT Discovery Server and a direct OMT target;
- bounded diagnostics and a downloadable support bundle;
- a two-step, rate-limited operating-system reboot action;
- an About page with version, copyright, project license, and third-party
  notices.

The Windows GUI has matching version, copyright, project license, and
third-party notices on its About tab.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Codebase reference](docs/CODEBASE_REFERENCE.md)
- [Configuration](docs/CONFIGURATION.md)
- [Setup](docs/SETUP.md)
- [Operations](docs/OPERATIONS.md)
- [Testing](docs/TESTING.md)
- [Diagnostics bundle](docs/DIAGNOSTICS_BUNDLE.md)

## License

Copyright (c) 2026 Matthew David Miller

The project-owned code is licensed under the [MIT License](LICENSE).
Third-party components retain their own terms in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt), with source availability
details in [THIRD_PARTY_SOURCE.md](THIRD_PARTY_SOURCE.md).
