#!/bin/bash
# Durable host-side file publication helpers.

host_publish_file() {
    local target="$1"
    local mode="$2"
    local owner="${3:-root}"
    local group="${4:-root}"
    local directory staged

    directory="$(dirname -- "${target}")"
    [[ -d "${directory}" && ! -L "${directory}" ]] || return 1
    staged="$(mktemp "${directory}/.$(basename -- "${target}").tmp.XXXXXX")"
    if ! {
        cat > "${staged}" &&
            chmod "${mode}" "${staged}" &&
            chown "${owner}:${group}" "${staged}" &&
            sync -f "${staged}" &&
            mv -fT -- "${staged}" "${target}" &&
            sync -d "${directory}"
    }; then
        rm -f -- "${staged}"
        return 1
    fi
}
