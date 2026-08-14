# Raspberry Pi OMT Client

Raspberry Pi OMT Client receives Open Media Transport (OMT) video and audio on
a Raspberry Pi and presents it directly on HDMI. It combines a bounded
Rust 2024 OMT receiver, direct DRM/KMS video output, ALSA audio, a hardened
Rust HTTPS Web GUI, and a portable native deployment GUI.

The supported appliance hosts are the Raspberry Pi 5, Raspberry Pi 4 Model B,
Raspberry Pi 3 Model A+/B/B+, and Raspberry Pi Zero 2 W, each running Alpine
Linux 3.24 aarch64 in persistent `sys` mode. Diskless Alpine, Raspberry Pi OS,
32-bit userspace, and every other board are not supported.

The receiver supports discovered source names and explicit `omt://host:port`
targets. Because it decodes VMX in software, each board has its own decode
ceiling — 1080p60 on a Pi 5, down to 720p60 on a Pi 3 or Zero 2 W. Larger or
faster input is reported as unsupported instead of being silently converted.
See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) for the table and how to
override it.

## Build and test

```bash
make install
make test-quick
make build-arm64
make build-deployer
make build-windows-deployer
```

The ARM64 build creates `omt-client-arm64.tar.gz`. The deployer build stages a
host-native egui application, CLI, and CycloneDX SBOM in
`.build/deployer-publish/`. On Linux, `make build-windows-deployer`
cross-compiles the same application for Windows x86-64 with mingw-w64 into
`.build/deployer-publish-windows/`. Linux and Windows hosts build the same
ARM64 appliance through the hermetic Dockerfile; `make install` provisions the
full local toolchain, including persistent ARM64 emulation on Linux x86-64.

## Install

Flash the official Alpine Raspberry Pi aarch64 image, add the Pi's SSH host
key to `known_hosts`, and run the native deployer. Factory Alpine answers as
`root` with no password. The Alpine view (or `rpi-omt-deploy alpine-setup`)
sets hostname, optional Wi-Fi, IPv4 DHCP, user `pi`, root/`pi` passwords, US
HTTPS apk mirrors, and persistent `sys` mode, then reboots. Deploy next.

A stock Alpine image has neither `bash` nor `sudo`, so it is bootstrapped once
before the appliance installer can run. The GUI uses the Alpine view's root
password for that `su` step; the CLI accepts `bootstrap_root_password`. The
shell target handles a root SSH account or a host that already has sudo/doas:

```bash
make deploy HOST=root@192.168.1.50
```

To install on the Pi itself, copy the nested files named in
`deploy/manifest-v3.txt` across, then run:

```bash
su -c '/bin/sh ./deploy/host/bootstrap.sh'   # once, on a stock image
sudo ./deploy/host/install.sh
```

See [docs/SETUP.md](docs/SETUP.md) for headless first boot, which the Raspberry
Pi Imager's presets do not cover for Alpine.

The installer verifies the OS and board, installs Alpine's Pi kernel, firmware,
DRM/ALSA tooling, Docker and OpenRC services, and applies appliance hardening
and low-memory defaults. It prints the authoritative HTTPS URL and initial
password instructions. It expects a clean Alpine installation and does not
migrate state from any other installation.

## Operator UI

The authenticated Web GUI provides:

- source selection, playback state, restart, and stop/clear;
- one optional OMT Discovery Server and a direct OMT target;
- bounded diagnostics and a downloadable support bundle;
- a two-step, rate-limited operating-system reboot action;
- an About page with version, copyright, project license, and third-party
  notices.

The native deployment GUI has matching version, copyright, project license, and
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
