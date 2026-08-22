# Setup Guide

> **Beta — not production ready.** Every `0.9.x` release is a beta: defaults,
> the supported-board matrix, and on-disk state may change between them without
> a migration path, and an upgrade may need the appliance re-provisioned rather
> than migrated. Deploy only to boards you can physically reach with a keyboard
> and monitor. **Version 1.0 will be the first production release.**

## Supported host

- Raspberry Pi 5 or Raspberry Pi 4 Model B;
- Alpine Linux 3.24 aarch64;
- persistent `sys` mode on SD, eMMC, or USB storage;
- connected DRM/KMS HDMI and ALSA HDMI playback devices;
- network reachability to OMT senders.

Each board has its own decode ceiling; see the video limit table in
[CONFIGURATION.md](CONFIGURATION.md). Both boards have two HDMI outputs.

Both boards also have a dual-band radio, and that is a support criterion. The
appliance is 5 GHz only: real-world testing showed 2.4 GHz cannot carry an OMT
stream, because its packet loss makes playback unusable however strong the
signal is. A board whose radio cannot leave 2.4 GHz therefore cannot run this
appliance, which is why the Pi Zero 2 W (BCM43436) and the Pi 3 tier — whose
Model B (BCM43438) has no 5 GHz radio — are rejected.

Diskless/data-mode Alpine is rejected because its RAM-backed root competes
with video decoding. Raspberry Pi OS, other distributions, 32-bit userspace,
the Pi 400/500, Compute Modules, the Pi 3 and Zero tiers, and every earlier
board are rejected before installer mutation.

Flash the official Alpine Raspberry Pi aarch64 image. The native deployer
handles `setup-alpine` and the persistent `sys` install: connect as `root`
with an empty SSH password (the factory image), fill hostname, optional
Wi-Fi, and the root/`pi` passwords on the Alpine view, then install. IPv4
uses DHCP on Ethernet and on Wi-Fi when an SSID is set or the image already
has a `wpa_supplicant.conf` (typical of a headless first boot). Leave the
SSID blank to keep that association. After reboot, connect as `pi` and
Deploy. Ethernet is strongly recommended when the board is not already on
Wi-Fi. The installer applies its own package update as well.

A factory image has **no `bash` and no `sudo`**. After Alpine setup the
native deployer still bootstraps those on first Deploy. Raspberry Pi OS
Imager headless presets do not apply to the Alpine image.

### Headless first boot

The Raspberry Pi Imager's headless presets do not apply to the Alpine image:
they write Raspberry Pi OS `userconf`/`firstrun` files that Alpine never reads,
so an Alpine card flashed that way still boots to a login prompt on the console
with no network. After Imager finishes, mount the card's `PIBOOT` partition and
open **SD card** in the Windows GUI or terminal deployer. Select that mounted
partition, enter the two-letter uppercase regulatory country, SSID, and Wi-Fi
password, then choose **Prepare SD card**. The deployer:

1. confirms the selected directory is an Alpine boot partition;
2. downloads
   [`macmpi/alpine-linux-headless-bootstrap`](https://github.com/macmpi/alpine-linux-headless-bootstrap/releases/tag/v1.9)
   v1.9 from its pinned commit over HTTPS, with a 1 MiB limit, and verifies its
   SHA-512;
3. writes `headless.apkovl.tar.gz` and a Linux-text `wpa_supplicant.conf` at
   the root of the partition.

The generated Wi-Fi file stores the SSID as hex and a derived WPA PSK, not the
plaintext passphrase. Its equivalent readable form is:

```ini
country=US
network={
    key_mgmt=WPA-PSK
    ssid=<hex-encoded-network-name>
    psk=<64-character-derived-psk>
}
```

Keep the boot media physically controlled, remove the bootstrap copy after the
installed system is reachable, and keep the installed
`/etc/wpa_supplicant/wpa_supplicant.conf` root-only. The appliance installer
preserves the network block and adds the control settings needed for later
Wi-Fi changes from the deployer.

`country=` is what lets the radio use 5 GHz, so set it to where the appliance
actually is. Leave it out and the appliance defaults to `US` rather than
running without one: an unset country leaves the kernel in the world domain,
which contains no channels 149-165 at all, and a dual-band Pi then silently
associates on 2.4 GHz at a fraction of the throughput an OMT 1080p stream
needs. A country you do declare is preserved everywhere and never overwritten.

### Bootstrapping bash and sudo

A stock Alpine image has **no `bash` and no `sudo`** — it ships busybox `ash`,
and while the `wheel` group exists, nothing grants it escalation. Both the
installer and the deploy transaction are bash scripts run through `sudo`, so
they cannot run on an untouched image.

The native deployer applications handle this automatically. `make deploy`
does too when the SSH account is root or already has an escalation path. They
detect the gap and run `deploy/host/bootstrap.sh`, which enables the
`community` repository, installs `bash` and `sudo`, and grants `wheel`
escalation through both `/etc/sudoers.d/omt-client` and
`/etc/doas.d/10-omt-client-wheel.conf`.

Automatic bootstrap needs the Alpine root password because Alpine has no
`sudo`, and although it ships the `doas` binary, every rule in the packaged
`/etc/doas.conf` is commented out. The GUI uses the Root password from the
Alpine view for that one `su` step on first Deploy when the SSH account is
not root. The CLI accepts the same secret as `bootstrap_root_password` in
`--secrets-stdin`; interactive mode prompts for it. They allocate a PTY for
`su`, disable terminal echo before sending the secret, run the fixed
bootstrap script, and discard that root secret after the operation.

Connecting directly as `root` also works when the host's SSH policy permits
it. The shell-only `make deploy` path cannot accept a second root credential;
for that path, run the bootstrap once by hand:

```bash
scp deploy/host/bootstrap.sh <admin>@<ip>:/tmp/
ssh -t <admin>@<ip> "su -c '/bin/sh /tmp/bootstrap.sh'"
```

After bootstrap, `sudo` works and every later deploy needs only the SSH user's
sudo password.

## Rust deployment applications

Build the deployer for the current Linux or Windows host:

```bash
make test-setup
make build-deployer
```

A Linux workstation also publishes the Windows application without a Windows
machine:

```bash
make build-windows-deployer
```

That target cross-compiles with Rust's `x86_64-pc-windows-gnu` target and
stages both `rpi-omt-deploy.exe` and the egui `rpi-omt-deployer.exe` with the
license, notices, and CycloneDX SBOM.

The Linux package is different by design: `rpi-omt-deploy` and the terminal
`rpi-omt-deploy-tui`, both static musl binaries with no shared-library
dependencies at all. The same file runs on Ubuntu, Debian, RHEL, Fedora, Arch,
and Alpine, needs no runtime to be installed first, and runs over SSH -- which
is usually how a rack-mounted Pi is reached. There is no Linux GUI: egui would
`dlopen` the operator's glibc-linked graphics driver and tie the binary to a
glibc floor, which is exactly the portability this avoids.
`make install` provisions the cross toolchain along with the rest of the local
gate tooling.

Building on Windows itself still works: run the commands from a Bash
environment (Git Bash or MSYS2) with GNU Make and Rust 1.97.1,
Python 3, and Docker Desktop's Linux engine on `PATH`. Either path emits the
CLI and egui executables; the appliance build itself
still runs entirely in the pinned Linux containers.

### What the deployer carries

The deployer *is* the appliance. `crates/omt-deployer-core/build.rs` compiles
every member of `deploy/manifest-v3.txt` into the executable, the ARM64 image
archive included, so an operator receives one file and needs no checkout, no
archive copied in beside it, and no build tooling of any kind. The deployer
version and the capsule version cannot disagree, because they are the same
artifact.

Both applications report what they carry: the GUI on its Deploy and About
views, and the CLI without a connection at all.

```bash
rpi-omt-deploy check
rpi-omt-deploy prerequisites
```

`check` prints the member count, the archive's size, and its SHA-256 -- the
same digest the Pi's own `sha256sum` is held to during the deployment.
`prerequisites` has one satisfied row for an embedded deployment, because
there is nothing on the workstation for it to need.

### Rebuilding from a working tree

`--project <checkout>` is the developer override: it takes the whole capsule
from that tree instead of the embedded one, and `--rebuild-image` additionally
rebuilds the ARM64 archive there first. It is all-or-nothing on purpose --
mixing this binary's host scripts with a working tree's image would deploy a
combination nobody built. Only this path needs local tooling, and only then
does the Setup report list any:

| Entry | Needed for | Windows |
|---|---|---|
| Container engine | building and exporting the image | Docker Desktop, Linux engine running |
| POSIX shell | running the pinned `scripts/build-arm64.sh` | Git for Windows |
| GNU Make | the documented entry point on Linux | not used; the shell is called directly |
| Python 3 | generating deployer SBOMs and repository gates | required for a release package |
| Project source tree | the manifest-v3 capsule that is uploaded | the checkout of this repository |
| Appliance image archive | the image the Pi loads | built by `--rebuild-image`, or copied in |

```bash
rpi-omt-deploy --project . prerequisites
rpi-omt-deploy --project . prerequisites --install --check-emulation
rpi-omt-deploy setup-emulation
```

It exits 1 when a required entry is missing, naming each one. On Windows,
`--install` installs the entries that have a package through `winget`; Windows
raises an approval prompt for each. Git for Windows is found where it installs
even when it is not yet on the `PATH` of the running deployer. A `bash.exe`
under `System32` is deliberately ignored: that is the WSL launcher, which runs
in a file system where the project path does not exist.

`setup-emulation` makes the engine able to execute `linux/arm64`, which the
appliance build needs because the image installs packages during its own
build. Docker Desktop does not arrive with the ARM64 `binfmt_misc` handler
registered, and the registration lives in its Linux VM, so it is lost whenever
that VM restarts; a `--rebuild-image` deployment registers it again first. On
Linux the handler belongs in the host's own kernel, where
`make setup-arm64-emulation` installs it persistently and verifies it as root.

Run `.build/deployer-publish/bin/rpi-omt-deploy-tui` on Linux, or
`rpi-omt-deployer.exe` on Windows, with a Pi key already trusted in `~/.ssh/known_hosts` and
administrator SSH/sudo credentials. Nothing else: the capsule is inside it. The Connection view
accepts an optional sudo password and an optional alternate `known_hosts` path,
each with a Browse button; the CLI equivalents are the `sudo_password` field in
`--secrets-stdin` and `--known-hosts <path>`. Factory Alpine images accept `root`
with an empty SSH password; leave that field blank until Alpine setup has set
one. The Alpine view's root password is the bootstrap secret for first Deploy
when the SSH account is not root (`bootstrap_root_password` in the CLI). Connect validates Alpine 3.24
aarch64 and a supported device-tree model.
The Alpine view runs `setup-alpine` equivalent configuration and a persistent
`sys` install: hostname, optional Wi-Fi (a blank SSID keeps an existing
boot-partition association), DHCP for IPv4, user `pi` in `wheel`,
root and `pi` passwords, and US HTTPS apk mirrors. It erases the boot disk and
reboots. Deploy uploads, verifies, and installs the embedded capsule, naming
the archive it carries before it sends it; the only field is the remote
directory. Web GUI password
rotation is off by default; enable **Rotate the Web GUI password after deploy**
on that view to replace the generated credential as part of the same job.
Manage reads
container status/logs or restarts the OpenRC service through sudo for a
non-root SSH account. Restart uses the service boundary so it can also start a
freshly installed appliance whose container has not been created yet.
Manage also offers a confirmed operating-system reboot. A successful Deploy
already reboots to apply kernel, firmware, and KMS settings, so that reboot
does not need a separate SSH session. The same view can change the Web GUI
password or rename the appliance later without redeploying.
Wi-Fi updates the running
`wpa_supplicant` through its control socket and stores a derived WPA PSK rather
than sending the plaintext passphrase to a command line. Existing profiles are
preserved by default. Clear **Preserve existing Wi-Fi profiles** to leave only
the submitted profile; when immediate connection is enabled, the new profile
must associate before the old profiles are removed. Clear **Connect immediately
after saving** to prepare the appliance for a different location without trying
the new network. That pauses automatic association, so perform a deferred
replacement over Ethernet or expect a Wi-Fi SSH session to disconnect.

About shows `LICENSE` and `THIRD_PARTY_NOTICES.txt` from inside the executable:
`include_str!` compiles both texts in, so the page cannot go blank
because the binary was copied somewhere without them. The published packages
still carry the files as well, for anyone reading the package rather than
running it. The GUI puts each text behind a collapsing header; the terminal
application has no such widget, so About is one document there -- version,
capsule digest, keys, license, notices -- scrolled with PageUp/PageDown,
Up/Down, Home, and End.

The application does not otherwise rely on the working directory it happens to
inherit from a shell or a desktop shortcut, and it reads no file beside itself:
the capsule it deploys is compiled in the same way those legal texts are.

### Display scaling and window size

Fonts, spacing, and the initial window follow the display's content scale, so
the window opens at the same apparent size on a scaled 4K desktop as on a
1366x768 panel. The native window is not resized while it is being dragged:
resizing mid-move is what makes a window manager snap it back to the original
display. Fonts and spacing still follow the new display's scale; zoom remains
available if the result is smaller or larger than wanted. The opening window
is then fitted to the monitor it actually landed on, taking at most 90% of
its width and 85% of its height, so a heavily scaled panel -- 1366x768 at
200% scaling leaves only 683x384 points in total -- never gets a window
larger than itself. That opening fit retries until the compositor has applied
the size, stops if the window is dragged, and gives up after a short
wall-clock budget rather than a frame count.

The window is not centred by eframe. That path uses the primary monitor's
size as an absolute desktop position, which on Windows with several displays
(especially mixed scale factors) opens the window on the wrong monitor. The
position is left unset so Windows `CW_USEDEFAULT` and the Linux/macOS window
manager place it on the display the operator is using; the opening fit then
sizes it to that display in the window's own points.

The window can be dragged down to 420x320 points. Every view scrolls rather
than clipping, the navigation and the Manage buttons wrap, and labels move from
beside their fields to above them, so no control becomes unreachable at that
size. Forms stop widening at a readable column and stay centred, so a host name
does not get a field the width of a 4K desktop.

The status bar reports the display scale the deployer detected and the current
zoom. Zoom runs from 60% to 300% in steps of 10, through the `-`, `+`, and
`Reset` buttons or `Ctrl` with `+`, `-`, and `0` (`Cmd` on macOS); the buttons
and the shortcuts share one rule, so they cannot disagree. Zoom applies for the
session and is not written to disk.

If an X11 session reports the wrong scale -- the status bar shows a display
scale that does not match the desktop's setting -- set `Xft.dpi` in the X
resources, or override it directly:

```bash
WINIT_X11_SCALE_FACTOR=2 .build/deployer-publish/bin/rpi-omt-deployer
```

Wayland and Windows report their scale per monitor and need no override.

## CLI deployment

Prepare a freshly flashed, mounted Alpine boot partition before the Pi's first
boot. The Wi-Fi password uses the same bounded stdin secret channel as remote
Wi-Fi management and never appears in process arguments:

```bash
printf '%s\n' '{"wifi_password":"..."}' | \
rpi-omt-deploy --secrets-stdin prepare-sd \
    --boot-directory /run/media/$USER/PIBOOT --country US --ssid studio
```

On Windows, `--boot-directory` is the mounted drive root, for example `E:\\`.
The command needs network access to download the pinned overlay, but it needs
no Pi connection and no project checkout.

```bash
make build-arm64
make deploy HOST=admin@192.168.1.50
```

On a factory Alpine image, install persistent sys mode first. The image
answers as `root` with an empty password, so `password` is sent as an empty
string — omitting the field entirely is what "no password" means to the
client, and it rejects that as a missing credential:

```bash
printf '%s\n' '{"password":"","root_password":"...","pi_password":"..."}' | \
rpi-omt-deploy --host 10.1.20.210 --username root --secrets-stdin \
    alpine-setup --hostname omt-client --ssid studio
```

Leave `--ssid` off to keep a `wpa_supplicant.conf` the image already carries.
Alpine setup is not resumable: it sets the root and `pi` passwords early, so a
run that fails partway leaves the board on those passwords rather than the
factory's empty one. Nothing is committed to disk until the install itself, so
power cycling returns a factory image to its original state.

`deploy/manifest-v3.txt` is the authoritative capsule. It includes the image,
Compose definition, host scripts, OpenRC definitions, shared validation rules,
transaction helper, and legal files. It is also the list the deployer's build
script embeds, so the file remains the single definition of the set whether the
bytes come from this binary or from a `--project` tree. The deployment clients
hash every member, verify every remote SHA-256, and promote the complete set
through the durable v3 transaction journal.

Its nested-path boundary is:

```text
deploy/compose.yml
deploy/host/bootstrap.sh
deploy/host/setup-sys.sh
deploy/host/set-hostname.sh
deploy/host/install.sh
deploy/host/uninstall.sh
deploy/host/host-diagnostics.sh
deploy/host/host-event-watcher.sh
deploy/host/host-reboot.sh
deploy/lib/reboot-request.sh
deploy/lib/board-profile.sh
deploy/lib/hdmi-config.sh
deploy/lib/host-validation.sh
deploy/lib/publication.sh
deploy/lib/service-install.sh
deploy/openrc/omt-client
deploy/openrc/omt-client-avahi-proxy
deploy/openrc/omt-client-host-diagnostics
deploy/openrc/omt-client-reboot
deploy/transaction.sh
deploy/manifest-v3.txt
LICENSE
THIRD_PARTY_NOTICES.txt
THIRD_PARTY_SOURCE.md
```

## Installer behavior

Run `sudo ./deploy/host/install.sh`. Before mutation it verifies Alpine 3.24,
aarch64, a supported board model, a persistent root filesystem, safe paths, and
the complete capsule. It expects a clean Alpine installation.

`--max-video` overrides the board's decode ceiling and `--hdmi-video` forces a
connector mode; both are retained in `/etc/omt-client/installer.conf` and
`auto` restores the default. Run `install.sh --help` for the accepted forms.

The installer then:

1. pins apk repositories to reputable US HTTPS mirrors, runs `apk update` and `apk upgrade --available` so the host is on the
   latest Alpine 3.24 packages, then installs `linux-rpi`, Raspberry Pi boot
   firmware, Broadcom firmware, ALSA/DRM tools, Docker/Compose, Avahi/D-Bus,
   nftables, inotify, `wpa_supplicant`, and zram support;
2. applies kernel/network sysctls, SSH forwarding/session safeguards, bounded
   Docker logs, daemon no-new-privileges, BPF JIT constant blinding, zram swap,
   a 128 MiB container cap, a 64-PID cap, and bounded file descriptors, shared
   memory, and tmpfs mounts. SSH logins are limited to `root` (keys only) and
   members of the administrative `wheel` group. IPv4 reverse-path filtering is
   pinned, IPv6 router advertisements and SLAAC are refused, apk repositories are
   pinned to US HTTPS mirrors, onboard Bluetooth is disabled, the CPU governor is
   pinned to `performance`, Wi-Fi power save is pinned off, and time
   synchronization is enabled;
3. installs a default-deny nftables input policy allowing established traffic,
   loopback, ICMP/IPv6 neighbor discovery, DHCP, mDNS, SSH, and the Web port;
4. loads the ARM64 image, prepares the persistent volume and least-privilege
   Avahi/diagnostics/reboot channels;
5. enables full KMS and HDMI audio in Alpine's active `usercfg.txt`, with an
   optional forced connector mode in the active cmdline file;
6. installs and enables the OpenRC container, Avahi proxy, diagnostics watcher,
   reboot watcher, and persistent `wpa_supplicant` control services. Existing
   Wi-Fi network blocks are preserved and the config remains root-only.

The installer never adds an operator to Docker's root-equivalent `docker`
group. Existing Docker daemon JSON is merged with the appliance security/log
policy. The native deployers and `make deploy` reboot the Pi after a successful
install so the updated kernel, firmware, and KMS settings are used; they wait
for SSH and the appliance to return. A local interactive `install.sh` still
prompts before rebooting.

## First use

Sign in at the HTTPS URL printed by the installer. After a native or `make
deploy` install, the deployer waits for the appliance and prints the first-start
Web GUI password when the container logs still hold it. The container generates
a random password on its first successful start and prints it once. On the Pi,
retrieve it with:

```bash
sudo sh -c '. /etc/conf.d/omt-client; docker compose --env-file "$OMT_COMPOSE_ENV_FILE" -f "$OMT_COMPOSE_FILE" logs omt-client' \
  | sed -n '/Web UI password/,+1p'
```

The line after `Web UI password (save this now):` is the password. Store it in
a password manager before Docker's bounded logs rotate. The persistent
`web_password` file contains only a PBKDF2-SHA256 hash; it cannot be used to
recover the plaintext, and upgrading or redeploying preserves that hash rather
than generating another password.

The desktop deployment application's **Logs** action and the CLI deployer's
`logs` command show the same container output, so they can also be used while
the first-start message is still retained.

Password rotation is optional. On the desktop deployer's **Deploy** view, enable
**Rotate the Web GUI password after deploy**, enter and confirm a 12-128 byte
password, then deploy. The same change is available later from **Manage**
without redeploying: enter and confirm the password and select
**Change Web GUI password**. Either path restarts the appliance and signs out
every Web session.
The password travels over SSH stdin and only its PBKDF2-SHA256 hash is
persisted. The CLI `deploy` command does not rotate the credential; use
`web-password` for that explicit operation.

Select a discovered source or save a direct target such as
`omt://192.168.1.60:6400`.

## Renaming an installed appliance

The name in the Web GUI header, in every page title, and in the `<name>.local`
mDNS record is the host's own hostname. Alpine setup writes it when the board
is first installed; **Manage -> Appliance hostname** changes it afterwards, as
does the CLI:

```bash
printf '%s\n' '{"password":"...","sudo_password":"..."}' | \
rpi-omt-deploy --host 10.1.20.223 --username pi --secrets-stdin \
    hostname --name studio-pi-2
```

The name is one DNS label of 1-63 characters: letters, digits, and inner
hyphens, the same rule Alpine setup applies. It is written to `/etc/hostname`,
the running kernel, the loopback entry in `/etc/hosts`, and the DHCP client
identity in `/etc/network/interfaces`, and Avahi is restarted so
`<name>.local` resolves immediately.

The appliance container is then recreated, which stops playback for a few
seconds. That recreation is not optional: Docker writes a host-network
container's `/etc/hostname` once, when the container is created, so a running
container keeps the name the host had at that moment however many times the
process inside it is restarted. An appliance that is deliberately stopped --
between a first install and its reboot, for instance -- is left stopped and
picks the name up when it starts.

The new name reaches DHCP-registered DNS at the next lease renewal or reboot;
nothing here renews a lease, because this operation usually runs over the SSH
session that lease is carrying.

## Upgrade and uninstall

Deploy a complete newer manifest-v3 capsule to the same directory. Persistent
credentials, sessions, TLS material, and source state remain in
`omt-config-v3`. Installer state kept outside the capsule is preserved too: the
saved HDMI mode and video ceiling, the Web credential hash, and the appliance's
hostname. The appliance images an update replaces are removed once the new one
is loaded, so repeated updates do not accumulate on the SD card.

Run `sudo ./deploy/host/uninstall.sh` to remove owned OpenRC services, host
state, firewall rules, image, and optionally the volume/install directory. The
shared Docker log policy and zram configuration remain as safe host defaults.
Wi-Fi services and network profiles also remain to avoid disconnecting the
operator from the host.
