#!/bin/bash
# Pure Alpine Pi usercfg/cmdline rules shared by the installer and its tests.
#
# These functions never touch /boot themselves: they take the current document
# and print the intended one. The installer stages and renames the result. That
# split is deliberate — the two documents below decide whether a Pi boots at
# all, so their rules have to be exercisable without a Pi.

# Accept "auto" or a KMS connector and mode such as HDMI-A-1:1920x1080@60.
host_validate_hdmi_video_mode() {
    local mode="$1"
    local dimensions width height refresh

    [[ "${mode}" == "auto" ]] && return 0
    [[ "${mode}" =~ ^HDMI-A-[12]:[1-9][0-9]{2,3}x[1-9][0-9]{2,3}@[1-9][0-9]{1,2}$ ]] || \
        return 1
    dimensions="${mode#*:}"
    width="${dimensions%%x*}"
    dimensions="${dimensions#*x}"
    height="${dimensions%%@*}"
    refresh="${dimensions#*@}"
    (( 10#${width} >= 320 && 10#${width} <= 7680 )) && \
        (( 10#${height} >= 200 && 10#${height} <= 4320 )) && \
        (( 10#${refresh} >= 23 && 10#${refresh} <= 240 ))
}

# Rewrite config.txt on stdin, replacing this product's managed block.
#
#   $1 board id, as printed by host_board_profile; defaults to pi5
#
# `dtoverlay=vc4-kms-v3d` is correct on every supported board: the firmware
# substitutes the Pi 5 variant itself. `gpu_mem` is emitted only for the
# pre-Pi-5 boards, which still split RAM with the VideoCore -- under full KMS
# the V3D driver allocates from CMA instead, so the split is wasted RAM, and it
# is worth the most on the 512 MiB Zero 2 W. The Pi 5 has no such split and
# ignores the setting, so it is left out there rather than written and ignored.
# Onboard Bluetooth is disabled in the same block: the appliance has no use
# for it, and leaving the controller up is idle RAM and an extra radio. Pi 5
# uses `dtparam=krnbt=off`; the others use `dtoverlay=disable-bt`.
host_hdmi_config_txt() {
    local board_id="${1:-pi5}"
    local gpu_mem=""
    local bt_disable="dtoverlay=disable-bt"
    [[ "${board_id}" == "pi5" ]] || gpu_mem="gpu_mem=64"
    [[ "${board_id}" == "pi5" ]] && bt_disable="dtparam=krnbt=off"

    awk -v gpu_mem="${gpu_mem}" -v bt_disable="${bt_disable}" '
        function flush_pending_blanks() {
            while (pending_blanks > 0) {
                print ""
                pending_blanks--
            }
        }
        function emit_unmanaged(line) {
            if (line ~ /^[[:space:]]*$/) {
                pending_blanks++
                return
            }
            flush_pending_blanks()
            print line
        }
        /^# BEGIN OMT Client HDMI configuration$/ {
            in_managed = 1
            managed_count = 0
            next
        }
        /^# END OMT Client HDMI configuration$/ {
            in_managed = 0
            managed_count = 0
            next
        }
        in_managed {
            managed_lines[++managed_count] = $0
            next
        }
        { emit_unmanaged($0) }
        END {
            # A truncated older managed block must not erase unrelated settings
            # that happen to follow its missing end marker.
            if (in_managed) {
                for (line_number = 1; line_number <= managed_count; line_number++) {
                    emit_unmanaged(managed_lines[line_number])
                }
            }
            print ""
            print "# BEGIN OMT Client HDMI configuration"
            print "dtoverlay=vc4-kms-v3d"
            print "max_framebuffers=2"
            print "disable_fw_kms_setup=1"
            if (gpu_mem != "") {
                print gpu_mem
            }
            print bt_disable
            print "# END OMT Client HDMI configuration"
        }
    '
}

# Print the kernel command line with this product's video= token reconciled.
#
#   $1 current single-line cmdline
#   $2 previously installed video= token, if any
#   $3 desired video= token, if any
#   $4 desired connector name, if any
#
# Fails when the line already carries an unmanaged video= setting for the
# desired connector: overwriting a setting this product did not write could
# leave the operator with no picture and no record of what changed.
host_hdmi_cmdline_line() {
    local current="$1" previous_token="$2" desired_token="$3" desired_connector="$4"
    local token
    local -a tokens=() updated=()

    read -r -a tokens <<< "${current}"
    for token in "${tokens[@]}"; do
        if [[ -n "${previous_token}" && "${token}" == "${previous_token}" ]]; then
            continue
        fi
        if [[ -n "${desired_connector}" && "${token}" == "video=${desired_connector}:"* ]]; then
            echo "unmanaged ${desired_connector} video setting: ${token}" >&2
            return 1
        fi
        updated+=("${token}")
    done
    if [[ -n "${desired_token}" ]]; then
        updated+=("${desired_token}")
    fi
    printf '%s\n' "${updated[*]}"
}

# Print the kernel command line with the memory cgroup forced on.
#
#   $1 current single-line cmdline
#
# The Raspberry Pi firmware injects `cgroup_disable=memory` into every boot,
# which strips the memory controller out of /proc/cgroups entirely. Docker then
# cannot honour the container memory cap this appliance advertises, and the
# 256 MiB limit silently becomes no limit at all on a board where playback and
# the web UI share the RAM. `cgroup_enable=memory` re-enables the controller;
# the firmware token is dropped rather than left to argue with it, because the
# kernel applies both in order and the result would depend on injection order.
host_cmdline_memory_cgroup() {
    local current="$1"
    local token
    local -a tokens=() updated=()
    local enable_seen=false

    read -r -a tokens <<< "${current}"
    for token in "${tokens[@]}"; do
        case "${token}" in
            cgroup_disable=memory) continue ;;
            cgroup_enable=memory) enable_seen=true ;;
        esac
        updated+=("${token}")
    done
    [[ "${enable_seen}" == true ]] || updated+=("cgroup_enable=memory")
    printf '%s\n' "${updated[*]}"
}
