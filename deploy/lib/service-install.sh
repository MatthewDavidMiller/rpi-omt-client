#!/bin/bash
# OpenRC service installation helpers shared by Alpine host workflows.

host_publish_openrc_service() {
    local target="$1"
    host_publish_file "${target}" 0755 root root
}

host_publish_openrc_conf() {
    local target="$1"
    host_publish_file "${target}" 0600 root root
}

# Print a wpa_supplicant document that retains country and network settings but
# has exactly the global controls required by the root-run deployment client.
host_wpa_supplicant_config() {
    awk '
        BEGIN {
            print "ctrl_interface=/run/wpa_supplicant"
            print "ctrl_interface_group=wheel"
            print "update_config=1"
        }
        /^[[:space:]]*(ctrl_interface|ctrl_interface_group|update_config)[[:space:]]*=/ { next }
        { print }
    '
}

host_remove_openrc_services() {
    host_remove_openrc_services_at /etc/init.d "$@"
}

host_remove_openrc_services_at() {
    local openrc_root="$1"
    local service
    shift
    host_validate_safe_absolute_path "${openrc_root}" || return 1
    [[ -d "${openrc_root}" && ! -L "${openrc_root}" ]] || return 1
    for service in "$@"; do
        [[ "${service}" =~ ^[A-Za-z0-9_.@-]+$ ]] || return 1
        rm -f -- "${openrc_root}/${service}"
    done
}
