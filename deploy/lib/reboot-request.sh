#!/bin/bash
# Pure request-body validation for the host reboot bridge.
# Sourced by deploy/host/host-reboot.sh and by unit tests.
#
# Expected caller-provided variables:
#   request            - raw request body (exactly four key=value lines)
#   now                - current epoch seconds
#   last_accepted_raw  - contents of the last-accepted file, or empty
#   MAX_AGE_SECONDS, MAX_FUTURE_SECONDS, COOLDOWN_SECONDS
#
# On success sets: version, action, request_id, requested_at_epoch
# On soft evaluation failure prints a stable reject reason to stdout and returns 1.

# Print a stable identity only for the expected fixed regular file. `%F` is
# deliberately not used here: GNU stat describes a zero-byte regular file as
# "regular empty file", which made a freshly installed result channel fail its
# first publication even though its inode, owner, group, and mode were correct.
reboot_fixed_file_identity() {
    local path="$1" expected_uid="$2" expected_gid="$3" expected_mode="$4"
    local identity
    [[ -f "${path}" && ! -L "${path}" ]] || return 1
    identity="$(stat -c '%d:%i:%u:%g:%a' -- "${path}" 2>/dev/null)" || return 1
    [[ "${identity}" =~ ^[0-9]+:[0-9]+:${expected_uid}:${expected_gid}:${expected_mode}$ ]] ||
        return 1
    printf '%s\n' "${identity}"
}

reboot_parse_request_body() {
    local line key value line_count=0 body="${request-}"
    local -A seen=()
    version=""
    action=""
    request_id=""
    requested_at_epoch=""
    while IFS= read -r line || [[ -n "${line}" ]]; do
        ((line_count += 1))
        [[ "${line}" == *=* ]] || {
            printf '%s\n' "invalid-request"
            return 1
        }
        key="${line%%=*}"
        value="${line#*=}"
        [[ -z "${seen[${key}]:-}" ]] || {
            printf '%s\n' "invalid-request"
            return 1
        }
        seen["${key}"]=1
        case "${key}" in
            version) version="${value}" ;;
            action) action="${value}" ;;
            request_id) request_id="${value}" ;;
            requested_at_epoch) requested_at_epoch="${value}" ;;
            *)
                printf '%s\n' "invalid-request"
                return 1
                ;;
        esac
    done <<< "${body}"

    [[ "${line_count}" -eq 4 && "${#seen[@]}" -eq 4 ]] || {
        printf '%s\n' "invalid-request"
        return 1
    }
    [[ "${request_id}" =~ ^[0-9a-f]{32}$ ]] || {
        printf '%s\n' "invalid-request"
        return 1
    }
    return 0
}

reboot_evaluate_request() {
    local current="${now-}"
    local max_future="${MAX_FUTURE_SECONDS-}"
    local max_age="${MAX_AGE_SECONDS-}"
    local cooldown="${COOLDOWN_SECONDS-}"
    local prior="${last_accepted_raw-}"
    [[ "${version-}" == "1" && "${action-}" == "reboot" ]] || {
        printf '%s\n' "invalid-request"
        return 1
    }
    [[ "${requested_at_epoch-}" =~ ^[1-9][0-9]*$ ]] || {
        printf '%s\n' "invalid-timestamp"
        return 1
    }
    (( 10#${requested_at_epoch} <= current + max_future )) || {
        printf '%s\n' "future-request"
        return 1
    }
    (( current - 10#${requested_at_epoch} <= max_age )) || {
        printf '%s\n' "stale-request"
        return 1
    }

    if [[ -n "${prior}" ]]; then
        local last_id="${prior%% *}"
        local last_epoch="${prior#* }"
        [[ "${last_id}" != "${request_id-}" ]] || {
            printf '%s\n' "replayed-request"
            return 1
        }
        if [[ "${last_epoch}" =~ ^[1-9][0-9]*$ ]] &&
           (( current - 10#${last_epoch} < cooldown )); then
            printf '%s\n' "cooldown-active"
            return 1
        fi
    fi
    return 0
}
