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
grep -qx '    ssid=74657374' <<< "${wpa_config}"
grep -qx '    psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
    <<< "${wpa_config}"

echo "Host install/uninstall helper behavior tests passed"
