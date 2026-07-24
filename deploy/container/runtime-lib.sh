#!/bin/bash
# Shared bounded-read and process-identity helpers.

export LC_ALL=C
OMT_BOUNDED_STATE_VALUE=""

omt_read_bounded_state() {
    local path="$1" maximum_bytes="$2" before after size
    OMT_BOUNDED_STATE_VALUE=""
    [[ "${maximum_bytes}" =~ ^[0-9]+$ ]] || return 1
    [[ -f "${path}" && ! -L "${path}" ]] || return 1
    before="$(stat -c '%d:%i:%s' -- "${path}" 2>/dev/null || true)"
    [[ "${before}" =~ ^[0-9]+:[0-9]+:[0-9]+$ ]] || return 1
    size="${before##*:}"
    (( size <= maximum_bytes )) || return 1
    OMT_BOUNDED_STATE_VALUE="$(head -c "$((maximum_bytes + 1))" -- "${path}")"
    after="$(stat -c '%d:%i:%s' -- "${path}" 2>/dev/null || true)"
    [[ "${before}" == "${after}" ]] || return 1
    (( ${#OMT_BOUNDED_STATE_VALUE} <= maximum_bytes ))
}

omt_bounded_regular_nonempty() {
    local path="$1" maximum_bytes="$2"
    omt_read_bounded_state "${path}" "${maximum_bytes}" || return 1
    iconv -f UTF-8 -t UTF-8 -- "${path}" >/dev/null 2>&1 || return 1
    [[ "${OMT_BOUNDED_STATE_VALUE}" =~ [^[:space:]] ]]
}

omt_proc_start_time() {
    local pid="$1" stat_line remainder
    [[ "${pid}" =~ ^[1-9][0-9]*$ ]] || return 1
    IFS= read -r stat_line < "/proc/${pid}/stat" || return 1
    remainder="${stat_line##*) }"
    awk '{print $20}' <<< "${remainder}"
}

omt_process_matches_command() {
    local pid="$1" command="$2" expected resolved arg resolved_arg
    [[ "${pid}" =~ ^[1-9][0-9]*$ && -n "${command}" ]] || return 1
    expected="$(readlink -f -- "${command}" 2>/dev/null || true)"
    [[ -n "${expected}" ]] || expected="${command}"
    resolved="$(readlink -f -- "/proc/${pid}/exe" 2>/dev/null || true)"
    [[ "${resolved}" == "${expected}" ]] && return 0
    [[ -r "/proc/${pid}/cmdline" ]] || return 1
    while IFS= read -r -d '' arg; do
        [[ "${arg}" == "${command}" || "${arg}" == "${expected}" ]] && return 0
        if [[ "${arg}" == /* ]]; then
            resolved_arg="$(readlink -f -- "${arg}" 2>/dev/null || true)"
            [[ "${resolved_arg}" == "${expected}" ]] && return 0
        fi
    done < "/proc/${pid}/cmdline"
    return 1
}
