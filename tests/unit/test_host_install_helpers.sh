#!/bin/bash
# Behavior tests for shared installer/uninstaller host helpers.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
mkdir -p "${CASE_DIR}/bin" "${CASE_DIR}/openrc" "${CASE_DIR}/publish"

# shellcheck source=deploy/lib/host-validation.sh
source "${ROOT}/deploy/lib/host-validation.sh"
# shellcheck source=deploy/lib/publication.sh
source "${ROOT}/deploy/lib/publication.sh"
# shellcheck source=deploy/lib/service-install.sh
source "${ROOT}/deploy/lib/service-install.sh"

host_validate_safe_absolute_path "${CASE_DIR}"
for unsafe in / relative /tmp/../root /tmp//root /tmp/.; do
    if host_validate_safe_absolute_path "${unsafe}"; then
        echo "unsafe host path accepted: ${unsafe}" >&2
        exit 1
    fi
done

regular="${CASE_DIR}/regular"
printf 'value' > "${regular}"
host_require_regular_file "${regular}"
ln -s "${regular}" "${CASE_DIR}/link"
if host_require_regular_file "${CASE_DIR}/link"; then
    echo "symlink accepted as a host input" >&2
    exit 1
fi

cat > "${CASE_DIR}/bin/chown" <<'EOF'
#!/bin/bash
[[ "${FAKE_CHOWN_FAIL:-0}" == "0" ]] || exit 19
printf '%s\n' "$*" >> "${FAKE_CHOWN_LOG}"
EOF
chmod 0755 "${CASE_DIR}/bin/chown"
FAKE_CHOWN_LOG="${CASE_DIR}/chown.log" \
PATH="${CASE_DIR}/bin:${PATH}" \
    host_publish_file "${CASE_DIR}/publish/unit.service" 0644 root root \
    <<< $'[Unit]\nDescription=isolated test'
grep -q '^root:root ' "${CASE_DIR}/chown.log"
grep -q '/\.unit\.service\.tmp\.' "${CASE_DIR}/chown.log"
grep -qx 'Description=isolated test' "${CASE_DIR}/publish/unit.service"
[[ "$(stat -c '%a' "${CASE_DIR}/publish/unit.service")" == 644 ]]
[[ -z "$(find "${CASE_DIR}/publish" -name '.unit.service.tmp.*' -print -quit)" ]]

if FAKE_CHOWN_FAIL=1 \
   FAKE_CHOWN_LOG="${CASE_DIR}/chown.log" \
   PATH="${CASE_DIR}/bin:${PATH}" \
   host_publish_file "${CASE_DIR}/publish/failing.service" 0644 root root \
       <<< 'must not publish'; then
    echo "publication unexpectedly succeeded after chown failure" >&2
    exit 1
fi
[[ ! -e "${CASE_DIR}/publish/failing.service" ]]
[[ -z "$(find "${CASE_DIR}/publish" -name '.failing.service.tmp.*' -print -quit)" ]]

# OpenRC service scripts must land executable and root-owned.
FAKE_CHOWN_LOG="${CASE_DIR}/unit-chown.log" \
PATH="${CASE_DIR}/bin:${PATH}" \
    host_publish_openrc_service "${CASE_DIR}/publish/managed-service" \
    <<< $'#!/sbin/openrc-run\ndescription="managed service"'
grep -q '^root:root ' "${CASE_DIR}/unit-chown.log"
[[ "$(stat -c '%a' "${CASE_DIR}/publish/managed-service")" == 755 ]]
grep -qx 'description="managed service"' "${CASE_DIR}/publish/managed-service"

touch "${CASE_DIR}/openrc/one" "${CASE_DIR}/openrc/two"
host_remove_openrc_services_at "${CASE_DIR}/openrc" one two
[[ ! -e "${CASE_DIR}/openrc/one" ]]
[[ ! -e "${CASE_DIR}/openrc/two" ]]
for unsafe_service in '../unsafe' '/etc/passwd' 'service name' '' 'service;rm'; do
    if host_remove_openrc_services_at "${CASE_DIR}/openrc" "${unsafe_service}"; then
        echo "unsafe OpenRC service name accepted: ${unsafe_service}" >&2
        exit 1
    fi
done
ln -s "${CASE_DIR}/openrc" "${CASE_DIR}/openrc-link"
for unsafe_root in "${CASE_DIR}/openrc-link" "${CASE_DIR}/../$(basename -- "${CASE_DIR}")" \
        relative / "${CASE_DIR}/absent"; do
    if host_remove_openrc_services_at "${unsafe_root}" one; then
        echo "unsafe OpenRC root accepted: ${unsafe_root}" >&2
        exit 1
    fi
done

# The uninstaller wrapper must default to Alpine's fixed init-script directory.
# Record the delegation in a separate shell: stubbing the callee here would let
# a wrong default remove units from the machine running this suite.
recorded_removal="$(
    bash -c '
        source "$1"
        host_remove_openrc_services_at() { printf "%s" "$*"; }
        host_remove_openrc_services one two
    ' helper "${ROOT}/deploy/lib/service-install.sh"
)"
[[ "${recorded_removal}" == "/etc/init.d one two" ]]

wpa_config="$(
    host_wpa_supplicant_config <<'EOF'
country=US
update_config=0
ctrl_interface=/old/socket
network={
    ssid=74657374
    psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
}
EOF
)"
[[ "$(grep -c '^ctrl_interface=/run/wpa_supplicant$' <<< "${wpa_config}")" -eq 1 ]]
[[ "$(grep -c '^ctrl_interface_group=wheel$' <<< "${wpa_config}")" -eq 1 ]]
[[ "$(grep -c '^update_config=1$' <<< "${wpa_config}")" -eq 1 ]]
grep -qx 'country=US' <<< "${wpa_config}"
[[ "$(grep -c '^country=' <<< "${wpa_config}")" -eq 1 ]]
grep -qx '    ssid=74657374' <<< "${wpa_config}"
grep -qx '    psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
    <<< "${wpa_config}"

# The country an operator already declared is where the appliance is, so a
# re-deploy must not relabel the radio.
wpa_config="$(
    host_wpa_supplicant_config CA <<'EOF'
country=GB
network={
    ssid=74657374
}
EOF
)"
grep -qx 'country=GB' <<< "${wpa_config}"
[[ "$(grep -c '^country=' <<< "${wpa_config}")" -eq 1 ]]

# A configuration with no country at all is the case that mattered: without one
# the kernel keeps the world domain, where channels 149-165 do not exist, so the
# board silently stays on 2.4 GHz and OMT video has a sixth of the throughput it
# needs. The default has to be written rather than left absent.
wpa_config="$(
    host_wpa_supplicant_config <<'EOF'
network={
    ssid=74657374
}
EOF
)"
grep -qx 'country=US' <<< "${wpa_config}"
grep -qx '    ssid=74657374' <<< "${wpa_config}"
[[ "$(host_wpa_supplicant_config CA < /dev/null | grep -c '^country=CA$')" -eq 1 ]]
# The globals still lead the document, ahead of every preserved network block.
[[ "$(host_wpa_supplicant_config <<< 'network={' | head -n 4 | tail -n 1)" == "country=US" ]]

# The 5 GHz band policy. Unlike the country this is not a default: 2.4 GHz is
# unsupported because its packet loss makes OMT playback unusable, so a scan
# list already in the document is replaced rather than carried through.
banded="$(
    host_wpa_supplicant_config <<'EOF'
freq_list=2412 2437 2462
network={
    ssid=74657374
}
EOF
)"
[[ "$(grep -c '^freq_list=' <<< "${banded}")" -eq 1 ]]
grep -qx "freq_list=${HOST_WIFI_FREQ_LIST}" <<< "${banded}"
grep -qv '2412' <<< "$(grep '^freq_list=' <<< "${banded}")"
for host_freq in ${HOST_WIFI_FREQ_LIST}; do
    [[ "${host_freq}" =~ ^5[0-9]{3}$ ]] || {
        echo "FAIL: the band policy must be 5 GHz only, found ${host_freq}" >&2
        exit 1
    }
done

# A per-network freq_list is a legal key that wpa_supplicant writes indented
# inside a profile. Only the global is band policy, so an indented one is
# carried through untouched rather than swallowed by the global's strip.
per_network="$(
    host_wpa_supplicant_config <<'EOF'
network={
    ssid=74657374
    freq_list=5180 5200
}
EOF
)"
grep -qx '    freq_list=5180 5200' <<< "${per_network}"
[[ "$(grep -c 'freq_list' <<< "${per_network}")" -eq 2 ]]

ipv4="$(
    host_primary_ipv4_from <<'EOF'
3: wlan0    inet 10.1.20.210/24 brd 10.1.20.255 scope global wlan0
4: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0
EOF
)"
[[ "${ipv4}" == "10.1.20.210" ]]
ipv4="$(
    host_primary_ipv4_from <<'EOF'
2: eth0    inet 127.0.0.2/8 brd 127.255.255.255 scope global eth0
4: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0
5: enp1s0    inet 192.0.2.10/24 brd 192.0.2.255 scope global enp1s0
EOF
)"
[[ "${ipv4}" == "192.0.2.10" ]]

echo "Host install/uninstall helper behavior tests passed"
