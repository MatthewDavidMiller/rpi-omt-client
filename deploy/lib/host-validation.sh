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

# First global IPv4 that is not a container/bridge address. `hostname -i` on
# Alpine often yields 127.0.1.1 from /etc/hosts, and `ip` lists docker0 beside
# the operator-facing NIC; neither belongs in the Web URL.
host_primary_ipv4_from() {
    awk '
        $2 ~ /^(docker|br-|cni|flannel|virbr|podman|veth)/ { next }
        {
            split($4, a, "/")
            if (a[1] ~ /^127\./) next
            if (!found) { print a[1]; found = 1 }
        }
    '
}

host_primary_ipv4() {
    ip -4 -o addr show scope global 2>/dev/null | host_primary_ipv4_from
}

host_require_regular_file() {
    local path="$1"
    [[ -f "${path}" && ! -L "${path}" ]]
}
