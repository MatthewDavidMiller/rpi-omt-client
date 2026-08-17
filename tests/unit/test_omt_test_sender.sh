#!/bin/bash
# Contract and lifecycle tests for the first-party Rust OMT A/V sender.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BUILD_SCRIPT="${PROJECT_ROOT}/scripts/build-omt-test-sender.sh"
RUN_SCRIPT="${PROJECT_ROOT}/scripts/omt-test-sender.sh"
FIREWALL_SCRIPT="${PROJECT_ROOT}/scripts/configure-omt-test-sender-firewall.sh"
SENDER_MANIFEST="${PROJECT_ROOT}/crates/omt-test-sender/Cargo.toml"
failures=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

require_literal() {
    local path="$1" text="$2" label="$3"
    grep -Fq -- "${text}" "${path}" && pass "${label}" || fail "${label}"
}

for script in "${BUILD_SCRIPT}" "${RUN_SCRIPT}" "${FIREWALL_SCRIPT}"; do
    [[ -x "${script}" ]] && pass "$(basename "${script}") is executable" ||
        fail "$(basename "${script}") is executable"
done
require_literal "${SENDER_MANIFEST}" 'omt-protocol = { version = "0.9.60", path = "../omt-protocol" }' \
    "sender uses the repository protocol crate"
dependency_count="$(awk '
    /^\[dependencies\]$/ { dependencies=1; next }
    /^\[/ { dependencies=0 }
    dependencies && /^[A-Za-z0-9_-]+[[:space:]]*=/ { count++ }
    END { print count + 0 }
' "${SENDER_MANIFEST}")"
if [[ "${dependency_count}" -eq 1 ]]; then
    pass "sender adds no third-party package dependency"
else
    fail "sender adds no third-party package dependency"
fi
require_literal "${BUILD_SCRIPT}" 'cargo build --locked --release' \
    "sender build is locked Rust"
require_literal "${BUILD_SCRIPT}" 'aarch64-unknown-linux-musl' \
    "Pi 4 and Pi 5 share an explicit ARM64 musl build"
require_literal "${BUILD_SCRIPT}" 'CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld' \
    "ARM64 sender uses the receiver-compatible linker"
require_literal "${FIREWALL_SCRIPT}" 'port port=6400-6600 protocol=tcp' \
    "firewalld covers the bounded direct OMT port range"
require_literal "${FIREWALL_SCRIPT}" 'tcp dport 6400-6600' \
    "nftables covers the bounded direct OMT port range on Alpine"

case_dir="$(mktemp -d)"
cleanup() {
    OMT_TEST_SENDER_HOME="${case_dir}/sender" "${RUN_SCRIPT}" stop >/dev/null 2>&1 || true
    rm -rf -- "${case_dir}"
}
trap cleanup EXIT

sender_home="${case_dir}/sender"
artifact="${sender_home}/artifacts/test/bin"
mkdir -p "${artifact}" "${case_dir}/test-bin"
cat > "${artifact}/omt-test-sender" <<'EOF'
#!/bin/bash
trap 'exit 0' TERM INT
echo "fixture sender started"
while :; do sleep 0.1; done
EOF
chmod +x "${artifact}/omt-test-sender"
ln -s artifacts/test "${sender_home}/current"

cat > "${case_dir}/test-bin/ss" <<EOF
#!/bin/bash
pid=\$(awk '{ print \$1 }' '${sender_home}/runtime/sender.pid')
echo "LISTEN 0 5 *:6400 *:* users:((\"omt-test-sender\",pid=\${pid},fd=9))"
EOF
chmod +x "${case_dir}/test-bin/ss"

if PATH="${case_dir}/test-bin:${PATH}" OMT_TEST_SENDER_HOME="${sender_home}" \
    "${RUN_SCRIPT}" start > "${case_dir}/start-output" &&
   grep -Fq 'omt://' "${case_dir}/start-output"; then
    pass "sender lifecycle starts and reports a direct OMT URI"
else
    fail "sender lifecycle starts and reports a direct OMT URI"
fi
if PATH="${case_dir}/test-bin:${PATH}" OMT_TEST_SENDER_HOME="${sender_home}" \
    "${RUN_SCRIPT}" status | grep -Eq '^running pid=[0-9]+ port=6400$'; then
    pass "sender lifecycle validates PID identity and listening port"
else
    fail "sender lifecycle validates PID identity and listening port"
fi
if PATH="${case_dir}/test-bin:${PATH}" OMT_TEST_SENDER_HOME="${sender_home}" \
    "${RUN_SCRIPT}" stop >/dev/null &&
   [[ "$(OMT_TEST_SENDER_HOME="${sender_home}" "${RUN_SCRIPT}" status 2>&1 || true)" == "stopped" ]]; then
    pass "sender lifecycle stops its managed process and clears state"
else
    fail "sender lifecycle stops its managed process and clears state"
fi
rm -f -- "${sender_home}/runtime/sender.log"
ln -s /dev/null "${sender_home}/runtime/sender.log"
if PATH="${case_dir}/test-bin:${PATH}" OMT_TEST_SENDER_HOME="${sender_home}" \
    "${RUN_SCRIPT}" start >/dev/null 2>&1; then
    fail "sender lifecycle refuses a symlink log"
else
    pass "sender lifecycle refuses a symlink log"
fi
rm -f -- "${sender_home}/runtime/sender.log"

cat > "${case_dir}/test-bin/sudo" <<'EOF'
#!/bin/bash
[[ "${1:-}" == "-n" ]] && shift
[[ "${1:-}" == "true" ]] && exit 0
exec "$@"
EOF
cat > "${case_dir}/test-bin/firewall-cmd" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" >> "${FIREWALL_LOG}"
case "$*" in *--query-rich-rule*) exit "${FIREWALL_QUERY_STATUS:-1}" ;; esac
EOF
chmod +x "${case_dir}/test-bin/sudo" "${case_dir}/test-bin/firewall-cmd"

if PATH="${case_dir}/test-bin:${PATH}" FIREWALL_LOG="${case_dir}/firewall.log" \
    "${FIREWALL_SCRIPT}" allow 10.1.20.210 >/dev/null &&
   [[ "$(grep -c -- '--add-rich-rule=' "${case_dir}/firewall.log")" -eq 2 ]] &&
   grep -Fq 'address=10.1.20.210/32' "${case_dir}/firewall.log" &&
   ! grep -Fq 'protocol=udp' "${case_dir}/firewall.log"; then
    pass "firewall allow installs only runtime and persistent source-scoped TCP rules"
else
    fail "firewall allow installs only runtime and persistent source-scoped TCP rules"
fi
if PATH="${case_dir}/test-bin:${PATH}" FIREWALL_LOG="${case_dir}/remove.log" \
    FIREWALL_QUERY_STATUS=0 "${FIREWALL_SCRIPT}" remove 10.1.20.210/32 >/dev/null &&
   [[ "$(grep -c -- '--remove-rich-rule=' "${case_dir}/remove.log")" -eq 4 ]]; then
    pass "firewall remove also cleans the obsolete discovery allowance"
else
    fail "firewall remove also cleans the obsolete discovery allowance"
fi
if PATH="${case_dir}/test-bin:${PATH}" FIREWALL_LOG="${case_dir}/broad.log" \
    "${FIREWALL_SCRIPT}" allow 0.0.0.0/0 >/dev/null 2>&1; then
    fail "firewall helper refuses an unrestricted source"
else
    pass "firewall helper refuses an unrestricted source"
fi

if ((failures > 0)); then
    echo "${failures} OMT test sender test(s) failed" >&2
    exit 1
fi
echo "All OMT test sender tests passed."
