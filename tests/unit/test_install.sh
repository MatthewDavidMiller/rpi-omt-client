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
require 'alpine-release.*3\.23' "installer must pin the validated Alpine branch"
require 'PI_MODEL.*Raspberry.*Pi.*5' "installer must require Raspberry Pi 5"
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
grep -Fq 'ctrl_interface=/run/wpa_supplicant' "${SERVICE_HELPERS}" || {
    echo "FAIL: Wi-Fi control socket must be enabled" >&2
    exit 1
}
grep -Fq 'update_config=1' "${SERVICE_HELPERS}" || {
    echo "FAIL: deployer Wi-Fi changes must be durable" >&2
    exit 1
}
require 'rc-update add wpa_supplicant boot' "wpa_supplicant must start through OpenRC"
forbid 'OMT_ALLOW_EMULATED_PI' "production Pi 5 validation must not have an environment bypass"

require '90-omt-client-hardening\.conf' "sysctl and SSH hardening must be managed"
require 'kernel\.unprivileged_bpf_disabled=1' "unprivileged BPF must be disabled"
require 'vm\.page-cluster=0' "zram swap-in must avoid unnecessary read-ahead"
require 'vm\.swappiness=100' "compressed swap must be preferred over reclaim pressure"
require 'ZRAM_MIB=\$\(\(MEMTOTAL_KIB / 4096\)\)' "zram must scale to one quarter of installed RAM"
require 'ZRAM_MIB >= 128' "zram must retain its low-memory minimum"
require 'ZRAM_MIB <= 512' "zram must retain its bounded maximum"
require 'rc-update add zram-init default' "zram must remain enabled across boots"
require 'no-new-privileges' "Docker daemon must default to no-new-privileges"
require 'userland-proxy.*false' "Docker userland proxy must be disabled"
require 'PermitRootLogin prohibit-password' "root password SSH must be disabled"
require 'table inet omt_client' "nftables appliance policy must be installed"
require 'policy drop' "firewall input must default deny"
require 'tcp dport.*SSH_PORT.*WEB_PORT' "firewall must retain SSH and Web access"
forbid 'usermod.*docker' "installer must not grant root-equivalent Docker group access"

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
require 'host_hdmi_config_txt' "KMS configuration must use tested rules"
require 'host_hdmi_cmdline_line' "forced connector mode must use tested rules"
require 'OMT_CONTAINER_MEMORY_LIMIT=256m' "low-RAM container cap must be explicit"
require 'STABLE_VOLUME="omt-config-v3"' "the persistent volume name must be fixed"
require 'chmod 0600.*HOST_REBOOT_REQUEST_FILE' "reboot request must be mode 0600"
require 'chmod 0640.*HOST_REBOOT_RESULT_FILE' "reboot result must be mode 0640"

echo "Alpine OMT installer contract tests passed"
