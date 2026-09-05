# Raspberry Pi OMT Client

Raspberry Pi OMT Client receives Open Media Transport (OMT) video and audio on
a Raspberry Pi and presents it directly on HDMI. It combines a bounded
Rust 2024 OMT receiver, direct DRM/KMS video output, ALSA audio, a hardened
Rust HTTPS Web GUI, and a portable native deployment GUI.

The supported appliance hosts are the Raspberry Pi 5 and Raspberry Pi 4
Model B, each running Alpine Linux 3.24 aarch64 in persistent `sys` mode. The
installer verifies the operating system, architecture, board model, and a
persistent root filesystem before it changes anything.

On Wi-Fi the appliance is 5 GHz only. Real-world testing showed 2.4 GHz cannot
carry an OMT stream: its packet loss makes playback unusable however strong the
signal is.

The receiver supports discovered source names and explicit `omt://host:port`
targets. Because it decodes VMX in software, each board has its own decode
ceiling — 1080p60 on a Pi 5, 1080p30 or 720p60 on a Pi 4. Larger or
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

The ARM64 build creates `omt-client-arm64.tar.gz`, and the deployer build
embeds it: run `make build-arm64` first, or the deployer build stops and says
so. Each executable carries the whole manifest-v3 capsule, so an operator needs
that one file and no checkout.

The Linux build stages a CLI and a terminal application, both linked fully
static against musl, plus a CycloneDX SBOM, in `.build/deployer-publish/`.
Static linking is why the Linux deployer is a terminal application rather than
a GUI: egui reaches the screen by `dlopen`ing libEGL, libGL, libX11, and
libwayland-client, which are the operator's graphics driver and are linked
against that machine's glibc. A terminal frontend opens nothing, so one binary
runs on every distribution -- glibc and musl alike -- and works over SSH.
`scripts/verify-linux-deployer.sh` reads that guarantee back out of the ELF
headers rather than trusting the build flags.

`make build-windows-deployer` cross-compiles the CLI and the egui application
for Windows x86-64 with mingw-w64 into `.build/deployer-publish-windows/`,
where `opengl32.dll` is a system library and the GUI costs nothing. Both
frontends run the same jobs from `omt-deployer-core`.

Docker or Podman is the only thing any of this needs from a workstation:
`make install` builds the toolbox image that carries every compiler, linter,
and scanner the gates use.

A finished version can be built, tagged, pushed, and published as a GitHub
Release entirely from the workstation with `make release`. This requires the
GitHub CLI authenticated by `gh auth login`; it does not use GitHub Actions.

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

Installing and running an appliance:

- [Setup](docs/SETUP.md) — install, upgrade, uninstall, headless first boot
- [Operations](docs/OPERATIONS.md) — dashboard, network, diagnostics, troubleshooting
- [Configuration](docs/CONFIGURATION.md) — environment, persistent files, HDMI, decode ceilings
- [Diagnostics bundle](docs/DIAGNOSTICS_BUNDLE.md) — support-bundle ZIP contract

Changing it:

- [Development](docs/DEVELOPMENT.md) — start here: layout, commands, invariants
- [Architecture](docs/ARCHITECTURE.md) — runtime design and the reasoning behind each bound
- [Codebase reference](docs/CODEBASE_REFERENCE.md) — file-to-responsibility map
- [Testing](docs/TESTING.md) — gates, hooks, release pipeline, hardware tier
- [OMT test sender](docs/OMT_TEST_SENDER.md) — first-party Rust test sender

## License

Copyright (c) 2026 Matthew David Miller

The project-owned code is licensed under the [MIT License](LICENSE).
Third-party components retain their own terms in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt), with source availability
details in [THIRD_PARTY_SOURCE.md](THIRD_PARTY_SOURCE.md).
