#!/bin/bash
# Small service installation helpers shared by host workflows.

host_publish_systemd_unit() {
    local target="$1"
    host_publish_file "${target}" 0644 root root
}

host_remove_systemd_units() {
    host_remove_systemd_units_at /etc/systemd/system "$@"
}

host_remove_systemd_units_at() {
    local systemd_root="$1"
    local unit
    shift
    host_validate_safe_absolute_path "${systemd_root}" || return 1
    [[ -d "${systemd_root}" && ! -L "${systemd_root}" ]] || return 1
    for unit in "$@"; do
        [[ "${unit}" =~ ^[A-Za-z0-9_.@-]+$ ]] || return 1
        rm -f -- "${systemd_root}/${unit}"
    done
}
