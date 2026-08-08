#!/bin/bash
# Supported Raspberry Pi boards and their decode ceilings.
#
# Like hdmi-config.sh, these functions never read /proc themselves: they take
# the device-tree model string and print the profile. The board decides whether
# the appliance installs at all and what video it will accept, so those rules
# have to be exercisable without four Raspberry Pis on the desk.
#
# A ceiling is a comma-separated list of shapes; a frame is admitted when it
# fits inside any one of them. That is what lets a Pi 4 take either 1080p30 or
# 720p60 without inventing a pixel-rate budget. `omt-receiver-core` parses the
# same string and owns admission at runtime; the tests below and its own unit
# tests are the two ends of that contract.
#
# The ceilings are engineering targets derived from core count and clock, not
# measurements. Confirm each on hardware with
# `cargo test --release -p vmx-decoder --test decode_bench -- --ignored` before
# release, and lower the tier here if a board cannot hold it.

# Absolute limits, matching omt-protocol's parse_video_header. No profile and
# no operator override may exceed these: they bound the decoder's allocations.
HOST_ABSOLUTE_MAX_WIDTH=1920
HOST_ABSOLUTE_MAX_HEIGHT=1080
HOST_ABSOLUTE_MAX_FPS=60
# More than any profile needs, which bounds the parse.
HOST_MAX_CEILING_SHAPES=4

# Print a board's profile as key=value lines, or fail for an unsupported model.
#
# The caller owns the error message, because the installer and the deployment
# probe word it differently. Matching is by prefix over the model string: the
# revision suffix varies per board batch, the Pi 3 B+ reports "Model B Plus"
# rather than "B+", and early Zero 2 W boards were silkscreened "Zero 2" with
# no W, so "Raspberry Pi Zero 2" is the prefix that covers the product.
host_board_profile() {
    local model="$1"
    local board_id board_label connectors ceiling

    # Each prefix ends at a word boundary. "Raspberry Pi 5"* would also match
    # "Raspberry Pi 500", a board this appliance has never been validated on.
    case "${model}" in
        "Raspberry Pi 5" | "Raspberry Pi 5 "*)
            board_id=pi5
            board_label="Raspberry Pi 5"
            connectors=2
            ceiling="1920x1080@60"
            ;;
        "Raspberry Pi 4 Model B"*)
            board_id=pi4
            board_label="Raspberry Pi 4 Model B"
            connectors=2
            # A72 at 1.8 GHz: either full-rate 720p or half-rate 1080p.
            ceiling="1920x1080@30,1280x720@60"
            ;;
        "Raspberry Pi 3 Model "*)
            board_id=pi3
            board_label="Raspberry Pi 3"
            connectors=1
            ceiling="1280x720@60"
            ;;
        "Raspberry Pi Zero 2" | "Raspberry Pi Zero 2 "*)
            board_id=pizero2w
            board_label="Raspberry Pi Zero 2 W"
            connectors=1
            ceiling="1280x720@60"
            ;;
        *)
            return 1
            ;;
    esac

    printf 'BOARD_ID=%s\n' "${board_id}"
    printf 'BOARD_LABEL=%s\n' "${board_label}"
    printf 'HDMI_CONNECTORS=%s\n' "${connectors}"
    printf 'VIDEO_CEILING=%s\n' "${ceiling}"
}

# The boards this appliance supports, for error messages and documentation.
host_supported_boards() {
    printf '%s\n' \
        "Raspberry Pi 5" \
        "Raspberry Pi 4 Model B" \
        "Raspberry Pi 3 Model A+/B/B+" \
        "Raspberry Pi Zero 2 W"
}

# Accept a ceiling: one or more WIDTHxHEIGHT@FPS shapes, comma separated.
#
# Every shape must be inside the absolute protocol limits, because the operator
# override reaches this function too and a ceiling above them would promise
# what the decoder's fixed allocations cannot deliver.
host_validate_video_ceiling() {
    local ceiling="$1"
    local shape width height fps
    local -a shapes=()

    [[ -n "${ceiling}" ]] || return 1
    # A trailing or doubled comma would otherwise read as a silently dropped
    # shape rather than as the malformed input it is.
    [[ "${ceiling}" != *,,* && "${ceiling}" != ,* && "${ceiling}" != *, ]] || return 1
    IFS=',' read -r -a shapes <<< "${ceiling}"
    (( ${#shapes[@]} >= 1 && ${#shapes[@]} <= HOST_MAX_CEILING_SHAPES )) || return 1

    for shape in "${shapes[@]}"; do
        [[ "${shape}" =~ ^[1-9][0-9]{1,3}x[1-9][0-9]{1,3}@[1-9][0-9]{0,2}$ ]] || return 1
        width="${shape%%x*}"
        height="${shape#*x}"
        height="${height%%@*}"
        fps="${shape#*@}"
        (( 10#${width} >= 16 && 10#${width} <= HOST_ABSOLUTE_MAX_WIDTH )) || return 1
        (( 10#${height} >= 16 && 10#${height} <= HOST_ABSOLUTE_MAX_HEIGHT )) || return 1
        (( 10#${fps} >= 1 && 10#${fps} <= HOST_ABSOLUTE_MAX_FPS )) || return 1
    done
}

# Print one profile field, or fail if the model is unsupported.
host_board_field() {
    local model="$1" field="$2" profile
    profile="$(host_board_profile "${model}")" || return 1
    sed -n "s/^${field}=//p" <<< "${profile}"
}
