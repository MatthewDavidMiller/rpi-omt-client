#!/bin/bash
# OpenRC service installation helpers shared by Alpine host workflows.

# Every 5 GHz channel a US station may use, as wpa_supplicant's `freq_list`.
#
# This is the appliance's band policy in one line. Real-world testing settled
# that 2.4 GHz cannot carry an OMT stream: the loss rate makes playback
# unusable regardless of signal strength, because the band is 20 MHz wide,
# shared with every other radio in the building, and the appliance is asking it
# for tens of megabits continuously. Restricting the scan is what makes that a
# property of the appliance rather than advice in a document -- a network the
# supplicant never scans for is one it cannot silently fall back to.
#
# DFS channels (52-144) are included because a station only follows an access
# point onto them; radar detection is the AP's duty, not the client's.
# Excluding them would rule out sites whose 5 GHz network sits in that range
# for exactly the congestion reasons that make 2.4 GHz unusable here.
HOST_WIFI_FREQ_LIST="5180 5200 5220 5240 5260 5280 5300 5320 5500 5520 5540 5560 5580 5600 5620 5640 5660 5680 5700 5720 5745 5765 5785 5805 5825"

host_publish_openrc_service() {
    local target="$1"
    host_publish_file "${target}" 0755 root root
}

host_publish_openrc_conf() {
    local target="$1"
    host_publish_file "${target}" 0600 root root
}

# Print a wpa_supplicant document that retains network settings but has exactly
# the global controls required by the root-run deployment client.
#
# The regulatory country is one of those controls rather than free text carried
# through, because without it the radio stays in the world domain. That domain
# omits U-NII-3 (channels 149-165) entirely and marks the rest of 5 GHz
# no-initiating-radiation, so a station with no country cannot join the band
# most 5 GHz access points in the US are configured on -- it never even scans
# them. A Pi that quietly stays on 2.4 GHz has roughly a sixth of the throughput
# an OMT 1080p stream needs, which is choppy video rather than an obvious fault.
#
# A country already in the document wins: it is the operator's declaration of
# where the appliance is, and re-deploying must not relabel a radio.
#
# `freq_list` is not negotiable the same way. Alpine's wpa_supplicant accepts
# the global spelling in the document but does not enforce it when roaming an
# existing network profile. Put the policy on every network as well, otherwise
# a dual-band SSID can silently fall back to 2.4 GHz after a disconnect.
host_wpa_supplicant_config() {
    local default_country="${1:-US}"
    awk -v default_country="${default_country}" -v freq_list="${HOST_WIFI_FREQ_LIST}" '
        /^[[:space:]]*country[[:space:]]*=/ {
            sub(/^[[:space:]]*country[[:space:]]*=[[:space:]]*/, "")
            if ($0 != "") { country = $0 }
            next
        }
        /^[[:space:]]*(ctrl_interface|ctrl_interface_group|update_config)[[:space:]]*=/ { next }
        /^[[:space:]]*network[[:space:]]*=\{[[:space:]]*$/ {
            in_network = 1
            body[++lines] = $0
            next
        }
        /^[[:space:]]*freq_list[[:space:]]*=/ { next }
        in_network && /^[[:space:]]*\}[[:space:]]*$/ {
            printf_line = sprintf("    freq_list=%s", freq_list)
            body[++lines] = printf_line
            body[++lines] = $0
            in_network = 0
            next
        }
        { body[++lines] = $0 }
        END {
            print "ctrl_interface=/run/wpa_supplicant"
            print "ctrl_interface_group=wheel"
            print "update_config=1"
            printf "country=%s\n", (country != "" ? country : default_country)
            printf "freq_list=%s\n", freq_list
            for (index_ = 1; index_ <= lines; index_++) { print body[index_] }
        }
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
