#!/bin/bash
# Behavior tests for shared installer/uninstaller host helpers.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
mkdir -p "${CASE_DIR}/bin" "${CASE_DIR}/systemd" "${CASE_DIR}/publish"

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

touch "${CASE_DIR}/systemd/one.service" "${CASE_DIR}/systemd/two.path"
host_remove_systemd_units_at \
    "${CASE_DIR}/systemd" one.service two.path
[[ ! -e "${CASE_DIR}/systemd/one.service" ]]
[[ ! -e "${CASE_DIR}/systemd/two.path" ]]
if host_remove_systemd_units_at "${CASE_DIR}/systemd" '../unsafe'; then
    echo "unsafe systemd unit name accepted" >&2
    exit 1
fi

echo "Host install/uninstall helper behavior tests passed"
