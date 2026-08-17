#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
INSTALL="${ROOT}/deploy/host/install.sh"
SERVICE_HELPERS="${ROOT}/deploy/lib/service-install.sh"
EVENT_WATCHER="${ROOT}/deploy/host/host-event-watcher.sh"

bash -n "${INSTALL}"

require() {
    grep -Eq -- "$1" "${INSTALL}" || {
        echo "FAIL: $2" >&2
        exit 1
    }
}
forbid() {
    if grep -Eq -- "$1" "${INSTALL}"; then
        echo "FAIL: $2" >&2
        exit 1
    fi
}

require 'uname -m.*aarch64' "installer must require aarch64"
require 'ID:-.*alpine' "installer must require Alpine"
require 'alpine-release.*SUPPORTED_ALPINE_SERIES' "installer must pin the validated Alpine branch"
require 'SUPPORTED_ALPINE_SERIES=3\.24' "the validated Alpine branch is 3.24"
require 'host_board_profile "\$\{PI_MODEL\}"' "installer must gate on the shared board table"
require 'docker info' "the installer must wait for dockerd to serve before using it"
# `set -o pipefail` turns a writer's SIGPIPE into a script-killing 141 with no
# message, which is how the firewall step died silently on a real Pi 4.
forbid '\| *awk .*exit *\}' "a piped awk must drain its input, not exit on first match"
forbid '\| *head -n' "head closes the pipe early; under pipefail that aborts the installer"
require 'host_supported_boards' "an unsupported board must be told what is supported"
require 'BOARD_HDMI_CONNECTORS.*==.*"1".*HDMI-A-2' \
    "a single-output board must refuse a forced HDMI-A-2 mode"
require 'tmpfs\|overlay\|squashfs\|ramfs' "diskless roots must be rejected"
require 'persistent sys mode' "low-memory sys-mode policy must be explained"
require 'command -v sshd' "installer must require a hardenable OpenSSH service"

require 'apk add --no-cache' "dependencies must come from apk"
require 'linux-rpi' "Pi-patched Alpine kernel must be installed"
require 'linux-firmware-brcm' "Pi firmware must be installed"
require 'raspberrypi-bootloader' "Pi boot firmware must be installed"
require 'alsa-utils' "ALSA tools and profiles must be installed"
require 'libdrm-tests' "DRM diagnostics must be installed"
require 'docker-cli-compose' "Alpine Compose plugin must be installed"
require 'zram-init' "compressed swap must be installed"
require 'host_wpa_supplicant_config' "installer must apply the tested Wi-Fi configuration"
# The Pi 5 radio is dual-band, but the kernel cannot leave the world domain
# without this database, and the world domain has no channels 149-165 at all.
# A board stuck on 2.4 GHz reports a healthy association while delivering a
# fraction of the throughput an OMT 1080p stream needs.
require 'wireless-regdb' "the regulatory database must be installed for 5 GHz"
require 'iw reg set' "the declared country must reach the running radio"
# 2.4 GHz is unsupported: its packet loss makes OMT playback unusable. The
# restriction is written to the configuration and applies at the next
# association -- it must not tear down the link this installer is running over,
# because that link is often the deploy's own SSH session and the SSID may have
# no 5 GHz BSS to move to. Warn, do not act.
require 'WIFI_CURRENT_FREQ' "the installer must notice a 2.4 GHz association"
require 'associated on .*2\.4 GHz' "a 2.4 GHz board must be told what changes at reboot"
forbid 'wpa_cli .*(reassociate|reconfigure|disconnect)' \
    "the installer must not reassociate the link it is running over"
grep -Fq 'ctrl_interface=/run/wpa_supplicant' "${SERVICE_HELPERS}" || {
    echo "FAIL: Wi-Fi control socket must be enabled" >&2
    exit 1
}
grep -Fq 'update_config=1' "${SERVICE_HELPERS}" || {
    echo "FAIL: deployer Wi-Fi changes must be durable" >&2
    exit 1
}
require 'rc-update add wpa_supplicant boot' "wpa_supplicant must start through OpenRC"
forbid 'OMT_ALLOW_EMULATED_PI' "production board validation must not have an environment bypass"

require '90-omt-client-hardening\.conf' "sysctl and SSH hardening must be managed"
require 'kernel\.unprivileged_bpf_disabled=1' "unprivileged BPF must be disabled"
require 'net\.core\.bpf_jit_harden=2' "the BPF JIT must blind constants for every caller"
require 'kernel\.perf_event_paranoid=3' "unprivileged performance events must be disabled"
require 'kernel\.sysrq=0' "the kernel SysRq interface must be disabled"
require 'fs\.suid_dumpable=0' "privileged processes must not produce core dumps"
require 'net\.ipv4\.tcp_syncookies=1' "TCP SYN cookies must be enabled"
require 'net\.ipv4\.conf\.all\.rp_filter=1' "IPv4 reverse-path filtering must be pinned"
require 'net\.ipv4\.conf\.all\.arp_ignore=1' "ARP replies must be limited to the incoming interface"
require 'net\.ipv4\.conf\.all\.drop_gratuitous_arp=1' "gratuitous ARP must be dropped"
require 'net\.ipv6\.conf\.all\.accept_ra=0' "IPv6 router advertisements must be refused"
require 'net\.ipv6\.conf\.all\.autoconf=0' "IPv6 SLAAC must be disabled"
require 'net\.ipv6\.conf\.all\.accept_source_route=0' "IPv6 source routing must be disabled"
require 'BEGIN US HTTPS APK MIRRORS' "apk repositories must be pinned to US HTTPS mirrors"
require 'https://mirrors\.edge\.kernel\.org' "installer must use the kernel.org HTTPS mirror"
require 'rc-update add ntpd default' "time synchronization must be enabled"
require 'rfkill block bluetooth' "onboard Bluetooth must be blocked"
require 'omt-client-blacklist\.conf' "Bluetooth kernel modules must be blacklisted"
require 'omt-client-cpufreq.start' "the CPU governor must be pinned to performance"
require 'set power_save off' "Wi-Fi power save must be disabled"
require 'omt-client-action\.sh' "wpa_cli must re-apply Wi-Fi power-save off on associate"
require 'DOCKER_API_WAIT_SECONDS=90' "the installer Docker wait must match the OpenRC 90-second bound"
require 'host_primary_ipv4' "the Web URL must pick a global IPv4, not hostname -i"
forbid 'hostname -i' "hostname -i on Alpine often yields 127.0.1.1 from /etc/hosts"
forbid 'OMT_BOARD_ID=' "unused board-id compose variables must not be written"
require 'rm -rf /config/run' "upgrade leftover SD-backed run/ state must be removed"
require 'vm\.page-cluster=0' "zram swap-in must avoid unnecessary read-ahead"
require 'vm\.swappiness=100' "compressed swap must be preferred over reclaim pressure"
require 'ZRAM_MIB=\$\(\(MEMTOTAL_KIB / 4096\)\)' "zram must scale to one quarter of installed RAM"
require 'ZRAM_MIB >= 128' "zram must retain its low-memory minimum"
require 'ZRAM_MIB <= 512' "zram must retain its bounded maximum"
require 'rc-update add zram-init default' "zram must remain enabled across boots"
require 'no-new-privileges' "Docker daemon must default to no-new-privileges"
require 'userland-proxy.*false' "Docker userland proxy must be disabled"
require 'dockerd --validate --config-file' "merged Docker configuration must be validated before publication"
require 'PermitRootLogin prohibit-password' "root password SSH must be disabled"
require 'AllowGroups root wheel' "SSH must be limited to root and appliance administrators"
require 'DisableForwarding yes' "all unused SSH forwarding channels must be disabled"
require 'PermitUserRC no' "user-controlled SSH startup commands must be disabled"
require 'MaxSessions 4' "SSH session fan-out must be bounded"
require 'MaxStartups 3:30:10' "unauthenticated SSH connections must be bounded"
require 'chmod 0600 "\$\{COMPOSE_ENV_TMP\}"' "the root-owned Compose environment must not be world-readable"
# The appliance's accepts must land in the host's own input chain. A private
# table hooked at a lower priority is accepted there and then dropped by
# Alpine's stock chain, which took SSH and the web UI down with it.
require 'table inet filter' "nftables appliance policy must be installed"
forbid 'table inet omt_client' \
    "a private nftables table cannot override another table's drop policy"
require 'policy drop' "firewall input must default deny"
require 'tcp dport.*SSH_PORT.*WEB_PORT' "firewall must retain SSH and Web access"
forbid 'usermod.*docker' "installer must not grant root-equivalent Docker group access"
require 'sudo docker compose --env-file.*logs omt-client' \
    "password retrieval must work without Docker group membership"

require 'host_publish_openrc_service' "OpenRC services must publish atomically"
require 'for service in omt-client-avahi-proxy omt-client-host-diagnostics omt-client-reboot omt-client' "all OMT OpenRC services must be enabled"
require 'rc-update add "\$\{service\}" default' "OMT services must be enabled through OpenRC"
require 'host-event-watcher\.sh' "fixed request watchers must be installed"
require 'inotify-tools' "request watchers must avoid polling"
grep -Fq 'inotifywait --monitor' "${EVENT_WATCHER}" || {
    echo "FAIL: request watchers must keep a race-free persistent watch" >&2
    exit 1
}
forbid 'systemctl' "Alpine host integration must not call systemd"
forbid 'apt-get' "Alpine host integration must not call apt"

require 'usercfg\.txt' "Alpine boot customization must use usercfg.txt"
forbid 'touch "\$\{USERCFG_FILE\}"' "installer must not follow a usercfg symlink before validating it"
require 'host_hdmi_config_txt "\$\{BOARD_ID\}"' "KMS configuration must be board-aware"
require 'host_hdmi_cmdline_line' "forced connector mode must use tested rules"
require 'active regular boot cmdline file is required to enable the memory cgroup' \
    "the installer must not succeed when it cannot enable the enforced memory limit"
require 'host_validate_video_ceiling' "the decode ceiling must use tested rules"
require 'OMT_VIDEO_CEILING=%s' "the effective decode ceiling must reach the container"
require 'MAX_VIDEO=\$\{MAX_VIDEO\}' "the operator's ceiling choice must be retained"
require 'OMT_CONTAINER_MEMORY_LIMIT=128m' "low-RAM container cap must be explicit"
require 'upgrade --available' "every deploy must upgrade Alpine packages to the latest index"
require 'Alpine package update still running' \
    "apk must emit progress so the deployer idle timeout cannot abort a download"
require 'Stopping the appliance before updating Alpine packages' \
    "a live appliance must not race a docker package upgrade"
require 'STABLE_VOLUME="omt-config-v3"' "the persistent volume name must be fixed"
require 'chmod 0600.*HOST_REBOOT_REQUEST_FILE' "reboot request must be mode 0600"
require 'the deployment client will reboot the Pi' \
    "non-interactive installs must not tell the operator to reboot by hand"

# xdg-dbus-proxy runs as nobody:${OMT_GID} and creates its socket inside this
# directory, so the group needs write, not just search. At 0750 the proxy died
# on "Error binding to address (GUnixSocketAddress): Permission denied" on
# every install, and the retry then collided with its own respawning orphan.
require 'install -d -m 2770 -o root -g "\$\{OMT_GID\}" "\$\{AVAHI_STATE_DIR\}"' \
    "the Avahi proxy socket directory must be writable by the proxy's group"
forbid 'install -d -m 0750 -o root -g "\$\{OMT_GID\}" "\$\{AVAHI_STATE_DIR\}"' \
    "a non-group-writable Avahi socket directory stops the proxy from binding"

# The unmanaged-connector guard has to fire before the install touches
# anything. It used to run with the boot-partition work, after the appliance
# and every OpenRC service had been stopped and the new image loaded, so the
# check that protects a hand-edited cmdline.txt was also what left the Pi down
# when it fired. Assert the order rather than the presence: this only regresses
# by the block moving.
first_line_of() {
    grep -nE -- "$1" "${INSTALL}" | head -1 | cut -d: -f1
}
GUARD_LINE="$(first_line_of 'contains an unmanaged connector mode')"
STOP_LINE="$(first_line_of 'Stopping the appliance before updating Alpine packages')"
LOAD_LINE="$(first_line_of '^docker load')"
[[ -n "${GUARD_LINE}" && -n "${STOP_LINE}" && -n "${LOAD_LINE}" ]] || {
    echo "FAIL: could not locate the connector guard, the stop, and the image load" >&2
    exit 1
}
(( GUARD_LINE < STOP_LINE )) || {
    echo "FAIL: the unmanaged connector guard must run before the appliance is stopped" >&2
    exit 1
}
(( GUARD_LINE < LOAD_LINE )) || {
    echo "FAIL: the unmanaged connector guard must run before the image is loaded" >&2
    exit 1
}
require 'Nothing has been changed and the appliance is still running' \
    "a rejected connector mode must say the appliance was left alone"

echo "Alpine OMT installer contract tests passed"
