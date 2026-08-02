#!/bin/bash
# Assertions that must execute inside a booted Raspberry Pi OS ARM64 VM.

set -euo pipefail

export LC_ALL=C
capsule_dir="${1:-/home/omtvm/rpi-omt-client}"
installer_log=/var/tmp/omt-vm-installer.log

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

pass() {
    echo "PASS: $*"
}

[[ -r /etc/os-release ]] || fail "/etc/os-release is unavailable"
# shellcheck source=/dev/null
source /etc/os-release
[[ -r /etc/rpi-issue ]] || fail "Raspberry Pi OS image marker is missing"
grep -qi 'Raspberry Pi' /etc/rpi-issue || fail "/etc/rpi-issue is not from Raspberry Pi OS"
dpkg-query -W -f='${db:Status-Status}\n' raspberrypi-sys-mods 2>/dev/null | \
    grep -Fxq installed || fail "raspberrypi-sys-mods is not installed"
[[ "$(uname -m)" == "aarch64" ]] || fail "guest kernel is not aarch64"
[[ "$(dpkg --print-architecture)" == "arm64" ]] || fail "guest userland is not arm64"
[[ "$(cat /proc/1/comm)" == "systemd" ]] || fail "PID 1 is not systemd"
[[ -r /proc/device-tree/model ]] || fail "emulated Raspberry Pi model is unavailable"
grep -aq 'Raspberry Pi' /proc/device-tree/model || fail "QEMU is not exposing a Raspberry Pi board"
pass "official Raspberry Pi OS ARM64 booted under full-system emulation"

for capsule_file in \
    omt-client-arm64.tar.gz \
    deploy/compose.yml \
    deploy/host/install.sh \
    deploy/host/host-diagnostics.sh \
    deploy/host/host-reboot.sh; do
    [[ -f "${capsule_dir}/${capsule_file}" ]] || fail "capsule is missing ${capsule_file}"
done

cd "${capsule_dir}"
printf 'n\n' | sudo ./deploy/host/install.sh 2>&1 | tee "${installer_log}"

docker_arch="$(sudo docker image inspect --format '{{.Architecture}}' omt-client)"
[[ "${docker_arch}" == "arm64" ]] || fail "installed image architecture is ${docker_arch}"
sudo docker volume inspect omt-config >/dev/null
pass "real installer loaded the ARM64 image and created persistent state"

for unit in \
    omt-client.service \
    omt-client-avahi-proxy.service \
    omt-client-host-diagnostics.service \
    omt-client-host-diagnostics.path \
    omt-client-reboot.service \
    omt-client-reboot.path; do
    sudo systemd-analyze verify "${unit}"
done
for unit in \
    omt-client.service \
    omt-client-avahi-proxy.service \
    omt-client-host-diagnostics.path \
    omt-client-reboot.path; do
    [[ "$(sudo systemctl is-enabled "${unit}")" == "enabled" ]] || \
        fail "${unit} is not enabled"
done
sudo systemctl is-active --quiet omt-client-host-diagnostics.path || \
    fail "diagnostics path unit is inactive"
sudo systemctl is-active --quiet omt-client-reboot.path || \
    fail "reboot path unit is inactive"
pass "installed systemd services verify and path watchers are active"

[[ "$(systemctl get-default)" == "multi-user.target" ]] || \
    fail "installer did not switch the full desktop image to headless boot"
for unit in display-manager.service lightdm.service; do
    if systemctl is-active --quiet "${unit}"; then
        fail "installer left ${unit} active"
    fi
done
pass "installer converted the full desktop image to its production headless target"

if [[ -c /dev/dri/card0 && -e /dev/snd ]] && \
   find /dev/snd -maxdepth 1 -type c -name 'pcmC*D*p' -print -quit | grep -q .; then
    sudo systemctl is-active --quiet omt-client.service || \
        fail "OMT service did not start with virtual DRM and ALSA devices"
    [[ -n "$(sudo docker compose --env-file deploy/.env -f deploy/compose.yml ps -q omt-client)" ]] || \
        fail "OMT container was not created"
    pass "container startup crossed the real systemd, Docker, DRM, and ALSA boundary"
else
    grep -q 'Container startup deferred; missing runtime devices:' "${installer_log}" || \
        fail "installer neither found virtual media devices nor explained startup deferral"
    pass "installer safely deferred startup because this Pi kernel lacks virtual media modules"
fi

diagnostics_id=0123456789abcdef0123456789abcdef
diagnostics_request=/var/lib/omt-client/diagnostics/request
diagnostics_report=/var/lib/omt-client/diagnostics/host-report.txt
diagnostics_owner="$(sudo stat -c '%u:%g' "${diagnostics_request}")"
sudo sh -c 'printf "version=1\nrequest_id=%s\nrequested_at_epoch=%s\ncapture_pcap=0\n" "$1" "$(date +%s)" > "$2"' \
    sh "${diagnostics_id}" "${diagnostics_request}"
sudo chown "${diagnostics_owner}" "${diagnostics_request}"
sudo chmod 0600 "${diagnostics_request}"
for _ in {1..60}; do
    if sudo grep -q "^request_id=${diagnostics_id}$" "${diagnostics_report}" 2>/dev/null; then
        break
    fi
    sleep 1
done
sudo grep -q '^version=1$' "${diagnostics_report}" || fail "diagnostics report is missing"
sudo grep -q "^request_id=${diagnostics_id}$" "${diagnostics_report}" || \
    fail "diagnostics path unit did not correlate its report"
pass "request-triggered host diagnostics ran through systemd"

# Exercise the reboot bridge with a correlated but invalid action. This reaches
# the real path unit and validator without rebooting the VM during the suite.
reboot_id=fedcba9876543210fedcba9876543210
reboot_request=/var/lib/omt-client/host-actions/reboot.request
reboot_result=/var/lib/omt-client/host-actions/reboot.result
reboot_owner="$(sudo stat -c '%u:%g' "${reboot_request}")"
sudo sh -c 'printf "version=1\naction=not-reboot\nrequest_id=%s\nrequested_at_epoch=%s\n" "$1" "$(date +%s)" > "$2"' \
    sh "${reboot_id}" "${reboot_request}"
sudo chown "${reboot_owner}" "${reboot_request}"
sudo chmod 0600 "${reboot_request}"
for _ in {1..20}; do
    if sudo grep -q "^request_id=${reboot_id}$" "${reboot_result}" 2>/dev/null; then
        break
    fi
    sleep 1
done
sudo grep -q "^request_id=${reboot_id}$" "${reboot_result}" || \
    fail "reboot path unit did not publish a correlated result"
sudo grep -q '^status=rejected$' "${reboot_result}" || fail "invalid reboot was not rejected"
sudo grep -q '^detail=invalid-request$' "${reboot_result}" || \
    fail "invalid reboot rejection detail changed"
pass "reboot path rejected an invalid action without rebooting"

echo "Raspberry Pi OS VM integration tests passed"
