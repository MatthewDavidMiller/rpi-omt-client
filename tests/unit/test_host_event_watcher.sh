#!/bin/bash
# Behavioral tests for deploy/host/host-event-watcher.sh.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
WATCHER="${ROOT}/deploy/host/host-event-watcher.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

fail() {
    echo -e "${RED}FAIL${NC}: $1" >&2
    exit 1
}

pass() {
    echo -e "${GREEN}PASS${NC}: $1"
}

run_as_root() {
    if [[ "${EUID}" -eq 0 ]]; then
        "$@"
        return
    fi
    if command -v unshare >/dev/null 2>&1 && unshare -r true >/dev/null 2>&1; then
        unshare -r -- "$@"
        return
    fi
    if command -v fakeroot >/dev/null 2>&1; then
        fakeroot -- "$@"
        return
    fi
    fail "root, unshare -r, or fakeroot is required for host-event-watcher behavioral tests"
}

tmpdir="$(mktemp -d)"
trap 'rm -rf -- "${tmpdir}"' EXIT

# Provide a deterministic inotifywait stand-in so the suite does not require the
# host package. It announces the watch, then emits one close_write event each
# time the watched file's mtime advances.
bin_dir="${tmpdir}/bin"
mkdir -p "${bin_dir}"
cat > "${bin_dir}/inotifywait" <<'EOF'
#!/bin/bash
set -euo pipefail
target=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --)
            shift
            target="${1:-}"
            break
            ;;
        *)
            shift
            ;;
    esac
done
[[ -n "${target}" ]] || exit 1
echo "Watches established." >&2
previous="$(stat -c '%Y:%s' -- "${target}" 2>/dev/null || echo none)"
while true; do
    sleep 0.05
    current="$(stat -c '%Y:%s' -- "${target}" 2>/dev/null || echo missing)"
    if [[ "${current}" != "${previous}" ]]; then
        previous="${current}"
        echo CLOSE_WRITE
    fi
done
EOF
chmod 755 "${bin_dir}/inotifywait"
export PATH="${bin_dir}:${PATH}"

request_file="${tmpdir}/request"
action_file="${tmpdir}/action.sh"
action_log="${tmpdir}/action.log"

printf '#!/bin/bash\necho ran >>%q\n' "${action_log}" > "${action_file}"
chmod 755 "${action_file}"
: > "${request_file}"
chmod 600 "${request_file}"

# Bad kind is validated before the root check.
if "${WATCHER}" bogus >/dev/null 2>&1; then
    fail "bad kind must exit non-zero"
fi
pass "rejects unknown kind"

# Missing request file.
rm -f -- "${request_file}"
if run_as_root env PATH="${PATH}" TMPDIR="${tmpdir}" \
    OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${request_file}" \
    OMT_DIAGNOSTICS_ACTION="${action_file}" \
    "${WATCHER}" diagnostics >/dev/null 2>&1; then
    fail "missing request file must be rejected"
fi
pass "rejects missing request file"

# Symlink request file.
: > "${tmpdir}/real-request"
ln -s "${tmpdir}/real-request" "${request_file}"
if run_as_root env PATH="${PATH}" TMPDIR="${tmpdir}" \
    OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${request_file}" \
    OMT_DIAGNOSTICS_ACTION="${action_file}" \
    "${WATCHER}" diagnostics >/dev/null 2>&1; then
    fail "symlink request file must be rejected"
fi
rm -f -- "${request_file}"
: > "${request_file}"
chmod 600 "${request_file}"
pass "rejects symlink request file"

# Non-executable action.
chmod 644 "${action_file}"
if run_as_root env PATH="${PATH}" TMPDIR="${tmpdir}" \
    OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${request_file}" \
    OMT_DIAGNOSTICS_ACTION="${action_file}" \
    "${WATCHER}" diagnostics >/dev/null 2>&1; then
    fail "non-executable action must be rejected"
fi
chmod 755 "${action_file}"
pass "rejects non-executable action"

# Non-empty request at start dispatches once, then close_write dispatches again.
# A failing action must not stop the watcher.
cat > "${action_file}" <<EOF
#!/bin/bash
echo ran >>'${action_log}'
if [[ \$(wc -l < '${action_log}') -eq 1 ]]; then
  exit 1
fi
EOF
chmod 755 "${action_file}"
: > "${action_log}"
printf 'payload\n' > "${request_file}"

watcher_log="${tmpdir}/watcher.log"
run_as_root env PATH="${PATH}" TMPDIR="${tmpdir}" \
    OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${request_file}" \
    OMT_DIAGNOSTICS_ACTION="${action_file}" \
    "${WATCHER}" diagnostics >"${watcher_log}" 2>&1 &
watcher_pid=$!

cleanup_watcher() {
    kill "${watcher_pid}" 2>/dev/null || true
    wait "${watcher_pid}" 2>/dev/null || true
}
trap 'cleanup_watcher; rm -rf -- "${tmpdir}"' EXIT

for _attempt in $(seq 1 200); do
    if [[ -s "${action_log}" ]]; then
        break
    fi
    sleep 0.05
done
[[ -s "${action_log}" ]] || {
    echo "watcher log:" >&2
    cat "${watcher_log}" >&2 || true
    fail "watcher did not dispatch on non-empty startup request"
}
pass "dispatches once for non-empty request at start"

# Ensure the fake watcher observes a distinct mtime/size change.
sleep 0.1
printf 'again\n' > "${request_file}"
for _attempt in $(seq 1 200); do
    if [[ "$(wc -l < "${action_log}")" -ge 2 ]]; then
        break
    fi
    sleep 0.05
done
[[ "$(wc -l < "${action_log}")" -ge 2 ]] || {
    echo "watcher log:" >&2
    cat "${watcher_log}" >&2 || true
    fail "watcher did not dispatch on close_write"
}
pass "dispatches on close_write after watch is established"

kill "${watcher_pid}" 2>/dev/null || true
wait "${watcher_pid}" 2>/dev/null || true
trap 'rm -rf -- "${tmpdir}"' EXIT
pass "survives action failure without exiting the watch loop"

echo -e "${GREEN}All host-event-watcher tests passed${NC}"
