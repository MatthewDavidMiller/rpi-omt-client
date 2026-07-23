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
third-party notices.

## CLI deployment

```bash
make build-arm64
make deploy HOST=pi@192.168.1.50
```

The flat deployment set contains:

- `omt-client-arm64.tar.gz`
- `docker-compose.yml`
- `install.sh`
- `uninstall.sh`
- `host-debug.sh`
- `host-reboot.sh`
- `LICENSE`
- `THIRD_PARTY_NOTICES.txt`
- `THIRD_PARTY_SOURCE.md`

`deploy-artifacts.txt` is the authoritative manifest and
`deploy-transaction.sh` provides crash recovery.

## Installer behavior

Run `sudo ./install.sh`. Before changing the host it rejects non-ARM64 systems,
unsafe install paths, missing capsule files, and any detected legacy NDI
service/state/container/volume. Legacy state is neither deleted nor migrated.

The installer:

1. configures headless boot and Docker;
2. loads the pinned OMT image and creates the external `omt-config` volume;
3. resolves container UID/GID and DRM/ALSA groups;
4. installs a filtered Avahi proxy and request-triggered diagnostics;
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
receiver/controller state, and bundle contents.

## Upgrade and uninstall

Deploy a complete newer nine-file capsule to the same OMT install directory.
The promotion journal prevents mixing artifact generations. Persistent OMT
state remains in `omt-config`.

Run `sudo ./uninstall.sh` to remove services and image. The script asks before
removing the install directory and `omt-config`. It does not remove any legacy
NDI installation or data.
