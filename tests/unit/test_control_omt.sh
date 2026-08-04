#!/bin/bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
CONTROL="${ROOT}/deploy/container/control-omt.sh"
RUNTIME="${ROOT}/deploy/container/runtime-lib.sh"
CASE_DIR="$(mktemp -d)"
STRAY_PIDS=()
cleanup() {
    local pid
    for pid in ${STRAY_PIDS[@]+"${STRAY_PIDS[@]}"}; do
        kill -KILL "${pid}" 2>/dev/null || true
    done
    rm -rf "${CASE_DIR}"
}
trap cleanup EXIT

mkdir -p "${CASE_DIR}/config/run"

# The fake receiver stands in for the native binary. Its path is what the
# controller's process-identity check is given, so every variant below must
# leave the interpreter on that pid rather than exec'ing something else.
install_receiver() {
    cat > "${CASE_DIR}/receiver"
    chmod 0755 "${CASE_DIR}/receiver"
}

install_receiver <<'EOF'
#!/bin/bash
exit 1
EOF

# Every invocation is bounded. The controller serialises on an exclusive flock,
# and a descriptor for it leaking into the receiver would hold that lock for the
# receiver's whole life -- so the regression is a hang, and an unbounded call
# here would hang this suite with it instead of naming the fault.
run_control() {
    OMT_CONFIG_DIR="${CASE_DIR}/config" \
    OMT_RUNTIME_LIB="${RUNTIME}" \
    OMT_RECEIVER_COMMAND="${CASE_DIR}/receiver" \
    START_OMT_CMD="${CASE_DIR}/receiver" \
        timeout 20 "${CONTROL}" "$@"
}

configure_target() {
    rm -f "${CASE_DIR}/config/source_target.json"
    printf '%s\n' '{"schema":1,"kind":"discovered","name":"Camera"}' \
        > "${CASE_DIR}/config/source_target.json"
}

if run_control invalid >/dev/null 2>&1; then
    echo "invalid controller action was accepted" >&2
    exit 1
fi
if run_control start >/dev/null 2>&1; then
    echo "controller started without a target" >&2
    exit 1
fi

configure_target
if run_control start >/dev/null 2>&1; then
    echo "controller trusted a receiver that exited immediately" >&2
    exit 1
fi
if run_control stop > "${CASE_DIR}/output" 2>&1; then
    echo "already-stopped controller returned success" >&2
    exit 1
fi
grep -q 'already stopped' "${CASE_DIR}/output"
if run_control status >/dev/null 2>&1; then
    echo "stopped controller returned success status" >&2
    exit 1
fi

printf '999999 1\n' > "${CASE_DIR}/config/run/omt.pid"
printf '{"state":"running"}\n' > "${CASE_DIR}/config/run/playback-status.json"
if run_control status >/dev/null 2>&1; then
    echo "stale PID record was trusted" >&2
    exit 1
fi
[[ ! -e "${CASE_DIR}/config/run/omt.pid" ]]
# A status the dead receiver left behind would otherwise pin the dashboard to
# "status stale" instead of the "stopped" the controller just reported.
[[ ! -e "${CASE_DIR}/config/run/playback-status.json" ]]

rm -f "${CASE_DIR}/config/source_target.json"
ln -s target "${CASE_DIR}/config/source_target.json"
if run_control start >/dev/null 2>&1; then
    echo "symlinked source target was trusted" >&2
    exit 1
fi
rm -f "${CASE_DIR}/config/source_target.json" "${CASE_DIR}/config/target"

# ─── A receiver that stays running ───────────────────────────────────────────
#
# Everything above watches the controller give up. These cases are the only
# ones that reach the PID record it writes, the identity check that guards
# every kill, and the stop paths, so a regression there would otherwise show up
# for the first time on a Pi.

# `sleep` as a child rather than `exec sleep`: the controller identifies the
# receiver by this script's path, which only survives on the pid while the
# interpreter does.
install_receiver <<'EOF'
#!/bin/bash
while true; do sleep 0.2; done
EOF
configure_target

run_control start > "${CASE_DIR}/output" 2>&1
grep -q '^Started OMT receiver (pid ' "${CASE_DIR}/output"
# Before anything else asks the controller for something. Its lock has to be
# free the moment it exits: a descriptor for it leaking into the receiver would
# leave this exclusive lock held for as long as playback runs, and every check
# below would then be measuring a 20-second timeout rather than the controller.
if ! flock -n -x "${CASE_DIR}/config/run/control.lock" true; then
    echo "the running receiver inherited the controller lock" >&2
    exit 1
fi
run_control status > "${CASE_DIR}/output" 2>&1
grep -qE '^running:[1-9][0-9]*$' "${CASE_DIR}/output"
RUNNING_PID="$(sed 's/^running://' "${CASE_DIR}/output")"
STRAY_PIDS+=("${RUNNING_PID}")
# The record is the pid paired with its start time; the pair is what makes a
# recycled pid unusable as a kill target.
read -r RECORD_PID RECORD_START < "${CASE_DIR}/config/run/omt.pid"
[[ "${RECORD_PID}" == "${RUNNING_PID}" ]]
[[ "${RECORD_START}" =~ ^[1-9][0-9]*$ ]]
[[ "$(stat -c '%a' "${CASE_DIR}/config/run/omt.pid")" == 600 ]]

# A second start must adopt the running receiver instead of launching a rival
# for the same exclusive DRM device.
run_control start > "${CASE_DIR}/output" 2>&1
grep -q "^OMT receiver already running:${RUNNING_PID}$" "${CASE_DIR}/output"

# A start time that does not match the live process means the pid was recycled,
# so the controller must not claim it.
printf '%s 1\n' "${RUNNING_PID}" > "${CASE_DIR}/config/run/omt.pid"
if run_control status >/dev/null 2>&1; then
    echo "a recycled pid was accepted as the managed receiver" >&2
    exit 1
fi
kill -KILL "${RUNNING_PID}" 2>/dev/null || true

run_control start > "${CASE_DIR}/output" 2>&1
read -r RUNNING_PID _ < "${CASE_DIR}/config/run/omt.pid"
STRAY_PIDS+=("${RUNNING_PID}")
run_control restart > "${CASE_DIR}/output" 2>&1
read -r RESTARTED_PID _ < "${CASE_DIR}/config/run/omt.pid"
STRAY_PIDS+=("${RESTARTED_PID}")
[[ "${RESTARTED_PID}" != "${RUNNING_PID}" ]]
! kill -0 "${RUNNING_PID}" 2>/dev/null

printf '{"state":"running"}\n' > "${CASE_DIR}/config/run/playback-status.json"
run_control stop > "${CASE_DIR}/output" 2>&1
grep -qx 'Stopped OMT receiver' "${CASE_DIR}/output"
! kill -0 "${RESTARTED_PID}" 2>/dev/null
[[ ! -e "${CASE_DIR}/config/run/omt.pid" ]]
[[ ! -e "${CASE_DIR}/config/run/playback-status.json" ]]

# ─── A receiver that refuses SIGTERM ─────────────────────────────────────────
#
# Worth the seconds it costs: SIGKILL is delivered, not completed, and the
# controller must not report a stop until the process has actually released
# /dev/dri to whatever `restart` launches next.
install_receiver <<'EOF'
#!/bin/bash
trap '' TERM
while true; do sleep 0.2; done
EOF

run_control start >/dev/null 2>&1
read -r STUBBORN_PID _ < "${CASE_DIR}/config/run/omt.pid"
STRAY_PIDS+=("${STUBBORN_PID}")
run_control stop > "${CASE_DIR}/output" 2>&1
grep -qx 'Force-stopped OMT receiver' "${CASE_DIR}/output"
! kill -0 "${STUBBORN_PID}" 2>/dev/null
[[ ! -e "${CASE_DIR}/config/run/omt.pid" ]]

# ─── Per-boot state on a tmpfs, the log on the persistent volume ─────────────
#
# The shipped image puts the lock, PID record, and status file on a tmpfs, so
# the controller has to take them from OMT_RUNTIME_DIR. Deriving them from
# OMT_CONFIG_DIR instead would silently keep writing status to flash while the
# receiver published it somewhere else entirely.
install_receiver <<'EOF'
#!/bin/bash
exit 1
EOF
RUNTIME_DIR="${CASE_DIR}/runtime"
mkdir -p "${RUNTIME_DIR}"
printf '999999 1\n' > "${RUNTIME_DIR}/omt.pid"
printf '{"state":"running"}\n' > "${RUNTIME_DIR}/playback-status.json"
if OMT_RUNTIME_DIR="${RUNTIME_DIR}" run_control status >/dev/null 2>&1; then
    echo "stale PID record in the runtime directory was trusted" >&2
    exit 1
fi
[[ ! -e "${RUNTIME_DIR}/omt.pid" ]]
[[ ! -e "${RUNTIME_DIR}/playback-status.json" ]]
[[ -e "${RUNTIME_DIR}/control.lock" ]]

# The receiver log is the only account of a failure that predates a restart, so
# it stays on the persistent volume rather than following the state to tmpfs.
# Clear the log this suite has already written, or the assertion below would
# pass on that leftover no matter where this start sends its output.
rm -f "${CASE_DIR}/config/receiver.log"
OMT_RUNTIME_DIR="${RUNTIME_DIR}" run_control start >/dev/null 2>&1 || true
[[ -e "${CASE_DIR}/config/receiver.log" ]]
[[ ! -e "${RUNTIME_DIR}/receiver.log" ]]

# ─── A launch that never becomes the receiver ───────────────────────────────
#
# start-omt.sh reads the saved target through Python before it execs the
# receiver, so for a moment every healthy start looks exactly like this. The
# controller has to treat "still not the receiver" as a failed start *and* take
# the process down with it: nothing else knows the pid -- no PID record is
# written -- so a survivor would hold /dev/dri against every later start with
# no way to stop it. Waiting on it instead would block this controller, holding
# its exclusive lock, for as long as that process lived.
install_receiver <<'EOF'
#!/bin/bash
printf '%s\n' "$$" > "${STRAY_PID_FILE}"
while true; do sleep 0.2; done
EOF
configure_target
STRAY_PID_FILE="${CASE_DIR}/stray.pid"
rm -f "${STRAY_PID_FILE}"
if OMT_CONFIG_DIR="${CASE_DIR}/config" \
   OMT_RUNTIME_LIB="${RUNTIME}" \
   OMT_RECEIVER_COMMAND="${CASE_DIR}/never-the-receiver" \
   START_OMT_CMD="${CASE_DIR}/receiver" \
   STRAY_PID_FILE="${STRAY_PID_FILE}" \
       timeout 20 "${CONTROL}" start >/dev/null 2>&1; then
    echo "a launch that never became the receiver was reported as started" >&2
    exit 1
fi
read -r ORPHAN_PID < "${STRAY_PID_FILE}"
STRAY_PIDS+=("${ORPHAN_PID}")
for _attempt in $(seq 1 50); do
    kill -0 "${ORPHAN_PID}" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "${ORPHAN_PID}" 2>/dev/null; then
    echo "the failed launch was left running with no PID record naming it" >&2
    exit 1
fi
[[ ! -e "${CASE_DIR}/config/run/omt.pid" ]]

# ─── PID record schema edges ────────────────────────────────────────────────
#
# Malformed records must never be treated as a live managed process. Oversized
# files, junk fields, and a zero pid are all fail-closed.
install_receiver <<'EOF'
#!/bin/bash
while true; do sleep 0.2; done
EOF
configure_target
run_control start >/dev/null 2>&1
read -r LIVE_PID LIVE_START < "${CASE_DIR}/config/run/omt.pid"
STRAY_PIDS+=("${LIVE_PID}")

printf '1 2 leftover\n' > "${CASE_DIR}/config/run/omt.pid"
if run_control status >/dev/null 2>&1; then
    echo "PID record with trailing junk was trusted" >&2
    exit 1
fi
printf '0 %s\n' "${LIVE_START}" > "${CASE_DIR}/config/run/omt.pid"
if run_control status >/dev/null 2>&1; then
    echo "zero pid was trusted" >&2
    exit 1
fi
python3 - <<PY
from pathlib import Path
path = Path("${CASE_DIR}/config/run/omt.pid")
path.write_bytes((("9 " + ("1" * 200) + "\n").encode()))
PY
if run_control status >/dev/null 2>&1; then
    echo "oversized PID record was trusted" >&2
    exit 1
fi
rm -f "${CASE_DIR}/config/run/omt.pid"
ln -s /proc/self "${CASE_DIR}/config/run/omt.pid"
if run_control status >/dev/null 2>&1; then
    echo "symlinked PID record was trusted" >&2
    exit 1
fi
rm -f "${CASE_DIR}/config/run/omt.pid"
# Restore a valid record so cleanup can stop the live receiver.
printf '%s %s\n' "${LIVE_PID}" "${LIVE_START}" > "${CASE_DIR}/config/run/omt.pid"
run_control stop >/dev/null 2>&1 || true

# Oversized receiver logs are rotated before the next start appends.
python3 - <<PY
from pathlib import Path
path = Path("${CASE_DIR}/config/receiver.log")
path.write_bytes(b"x" * (1024 * 1024 + 64))
PY
run_control start >/dev/null 2>&1 || true
LOG_SIZE="$(stat -c '%s' -- "${CASE_DIR}/config/receiver.log")"
(( LOG_SIZE <= 1024 * 1024 )) || {
    echo "receiver log was not rotated before append (${LOG_SIZE})" >&2
    exit 1
}
run_control stop >/dev/null 2>&1 || true

echo "OMT controller tests passed"
