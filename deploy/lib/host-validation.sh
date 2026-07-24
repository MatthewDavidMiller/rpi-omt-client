#!/bin/bash
# Shared host-side path and input validation.

host_validate_safe_absolute_path() {
    local path="$1"
    [[ "${path}" != "/" ]] || return 1
    [[ "${path}" =~ ^/[A-Za-z0-9._/-]+$ ]] || return 1
    [[ "${path}" != *"//"* ]] || return 1
    [[ "${path}" != */./* && "${path}" != */../* ]] || return 1
    [[ "${path}" != */. && "${path}" != */.. ]] || return 1
}

host_require_regular_file() {
    local path="$1"
    [[ -f "${path}" && ! -L "${path}" ]]
}
