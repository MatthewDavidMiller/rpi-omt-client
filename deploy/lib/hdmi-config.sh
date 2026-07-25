#!/bin/bash
# Pure HDMI boot-configuration rules shared by the installer and its tests.
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
host_hdmi_config_txt() {
    awk '
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
            print "[all]"
            print "dtoverlay=vc4-kms-v3d"
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
