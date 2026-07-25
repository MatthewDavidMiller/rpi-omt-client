# Setup Guide

## Requirements

- Raspberry Pi 5 running 64-bit Raspberry Pi OS
- connected DRM/KMS HDMI output and ALSA playback device
- network reachability to OMT senders
- ARM64 image produced by `make build-arm64`

## Windows GUI

Build the self-contained Windows x64 deployer:

```bash
make test-setup
make build-windows-deployer
```

Run `dist/rpi-omt-client-deployer-windows-x64.exe` on Windows 10/11 with
Docker Desktop, OpenSSH host trust, Pi SSH credentials, and this source tree.
The Deploy tab builds/uploads; Manage reads status/logs or restarts; Wi-Fi
updates NetworkManager; About displays version, copyright, license, and
third-party notices. The theme selector in the title row offers System (the
default, which follows Windows), Light, and Dark. While an action runs, the
activity row shows the state chip and an indeterminate progress bar; validation
problems appear in the message bar above it.

## CLI deployment

```bash
make build-arm64
make deploy HOST=pi@192.168.1.50
```

Manifest version 2 contains a variable number of normalized relative paths,
including:

- `omt-client-arm64.tar.gz`
- `deploy/compose.yml`
- `deploy/host/install.sh`
- `deploy/host/uninstall.sh`
- `deploy/host/host-diagnostics.sh`
- `deploy/host/host-reboot.sh`
- `deploy/lib/hdmi-config.sh`
- `deploy/lib/host-validation.sh`
- `deploy/lib/publication.sh`
- `deploy/lib/service-install.sh`
- `deploy/transaction.sh`
- `deploy/manifest-v2.txt`
- `LICENSE`
- `THIRD_PARTY_NOTICES.txt`
- `THIRD_PARTY_SOURCE.md`

`deploy/manifest-v2.txt` is the authoritative manifest and
`deploy/transaction.sh` provides durable nested-path crash recovery.

## Installer behavior

Run `sudo ./deploy/host/install.sh`. Before changing the host it rejects
non-ARM64 systems, unsafe install paths, missing capsule files, and any
detected legacy NDI service/state/container/volume. Legacy NDI state is neither
deleted nor migrated.

The installer:

1. configures headless boot and Docker;
2. loads the pinned OMT image and creates the external `omt-config` volume;
3. resolves container UID/GID and DRM/ALSA groups;
4. creates separate Avahi, diagnostics, and host-action directories and
   installs the filtered Avahi proxy plus request-triggered diagnostics;
5. pre-creates protected reboot request/result files and installs their
   root-owned systemd validator;
6. configures DRM/KMS and optional connector mode;
7. installs and enables `omt-client.service`.

It prints the authoritative HTTPS URL. Retrieve the generated first password
from the container log as shown in the installer summary.

## First use

Sign in, select a discovered source on Dashboard, or save a direct target such
as `omt://192.168.1.60:6400` on Network Settings. An optional Discovery Server
defaults to port 6399. Diagnostics can verify discovery, direct reachability,
receiver/controller state, and bundle contents. Raw packet capture is disabled
unless selected for an individual diagnostics download.

## Upgrade and uninstall

Deploy a complete newer manifest-v2 capsule to the same OMT install directory.
The clients first recover an installed v1 transaction with its own v1 manifest,
then use the staged v2 helper. The v2 journal stores its own manifest, so
rollback never depends on the next release's artifact list. Persistent OMT
state remains in `omt-config`; credentials, sessions, TLS material, and source
target schema 1 are preserved.

Run `sudo ./deploy/host/uninstall.sh` to remove services and image. The script
asks before removing the install directory and `omt-config`. It does not
remove any legacy NDI installation or data.
