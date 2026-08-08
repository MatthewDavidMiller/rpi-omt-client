#!/bin/bash
# Behavior tests for the supported-board table and its decode ceilings.
#
# The model string decides whether the appliance installs at all and what video
# it will accept, and none of the four boards is on this workstation, so every
# rule is exercised here rather than discovered on someone's desk.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
# shellcheck source=deploy/lib/board-profile.sh
source "${ROOT}/deploy/lib/board-profile.sh"

failures=0
fail() {
    echo "FAIL: $*" >&2
    failures=$((failures + 1))
}

expect_equal() {
    local label="$1" expected="$2" actual="$3"
    [[ "${expected}" == "${actual}" ]] ||
        fail "${label}: expected [${expected}], got [${actual}]"
}

expect_board() {
    local model="$1" board_id="$2" connectors="$3" ceiling="$4"
    local profile
    profile="$(host_board_profile "${model}")" || {
        fail "rejected a supported board: ${model}"
        return
    }
    expect_equal "${model} board id" "${board_id}" \
        "$(sed -n 's/^BOARD_ID=//p' <<< "${profile}")"
    expect_equal "${model} connectors" "${connectors}" \
        "$(sed -n 's/^HDMI_CONNECTORS=//p' <<< "${profile}")"
    expect_equal "${model} ceiling" "${ceiling}" \
        "$(sed -n 's/^VIDEO_CEILING=//p' <<< "${profile}")"
}

# ─── The supported matrix ────────────────────────────────────────────────────

expect_board "Raspberry Pi 5 Model B Rev 1.0" pi5 2 "1920x1080@60"
expect_board "Raspberry Pi 4 Model B Rev 1.4" pi4 2 "1920x1080@30,1280x720@60"
expect_board "Raspberry Pi 3 Model B Rev 1.2" pi3 1 "1280x720@60"
# The B+ reports "Model B Plus", never "Model B+".
expect_board "Raspberry Pi 3 Model B Plus Rev 1.3" pi3 1 "1280x720@60"
expect_board "Raspberry Pi 3 Model A Plus Rev 1.0" pi3 1 "1280x720@60"
expect_board "Raspberry Pi Zero 2 W Rev 1.0" pizero2w 1 "1280x720@60"
# Early Zero 2 W boards were silkscreened "Zero 2" with no W and are the same
# product, so the prefix must not require the W.
expect_board "Raspberry Pi Zero 2 Rev 1.0" pizero2w 1 "1280x720@60"

# Every profile's own ceiling has to satisfy the validator the operator
# override is checked against; otherwise a board could ship a default that no
# one is allowed to re-enter.
for model in \
    "Raspberry Pi 5 Model B Rev 1.0" \
    "Raspberry Pi 4 Model B Rev 1.4" \
    "Raspberry Pi 3 Model B Rev 1.2" \
    "Raspberry Pi Zero 2 W Rev 1.0"; do
    ceiling="$(host_board_field "${model}" VIDEO_CEILING)"
    host_validate_video_ceiling "${ceiling}" ||
        fail "profile ceiling is not a valid ceiling: ${model} -> ${ceiling}"
done

# ─── Everything else is refused ──────────────────────────────────────────────

# The 32-bit-only and unvalidated boards. A near miss matters most here: the
# original Zero W must not be caught by the Zero 2 prefix, and the Pi 2 must
# not be caught by a loosened Pi prefix.
for model in \
    "" \
    "Raspberry Pi Model B Plus Rev 1.2" \
    "Raspberry Pi 2 Model B Rev 1.1" \
    "Raspberry Pi Zero W Rev 1.1" \
    "Raspberry Pi Zero Rev 1.3" \
    "Raspberry Pi 400 Rev 1.0" \
    "Raspberry Pi 500 Rev 1.0" \
    "Raspberry Pi Compute Module 4 Rev 1.0" \
    "Raspberry Pi Compute Module 5 Rev 1.0" \
    "Orange Pi 5" \
    "Raspberry Pi" \
    "garbage"; do
    if host_board_profile "${model}" >/dev/null 2>&1; then
        fail "accepted an unsupported board: [${model}]"
    fi
done

# ─── Ceiling validation ──────────────────────────────────────────────────────

for ceiling in \
    "1920x1080@60" \
    "1280x720@60" \
    "1920x1080@30,1280x720@60" \
    "16x16@1" \
    "640x480@25,800x600@30,1280x720@50,1920x1080@30"; do
    host_validate_video_ceiling "${ceiling}" ||
        fail "rejected a supported ceiling: ${ceiling}"
done

# Anything above the absolute protocol limits is refused however it is spelled,
# because the decoder's allocations are fixed at 1920x1080 and the operator
# override reaches this validator too.
for ceiling in \
    "" \
    "1921x1080@60" \
    "1920x1081@60" \
    "1920x1080@61" \
    "3840x2160@30" \
    "1920x1080" \
    "1920X1080@60" \
    "1920x1080@60Hz" \
    "0x1080@60" \
    "1920x0@60" \
    "1920x1080@0" \
    "15x15@60" \
    "1920x1080@60," \
    ",1920x1080@60" \
    "1920x1080@60,,1280x720@30" \
    "1920x1080@60 1280x720@30" \
    "0640x480@30" \
    "640x480@25,800x600@30,1280x720@50,1920x1080@30,640x360@24" \
    "1920x1080@60; reboot" \
    '1920x1080@60$(reboot)'; do
    if host_validate_video_ceiling "${ceiling}"; then
        fail "accepted an unsupported ceiling: [${ceiling}]"
    fi
done

# ─── Field accessor ──────────────────────────────────────────────────────────

expect_equal "board label is human readable" "Raspberry Pi Zero 2 W" \
    "$(host_board_field "Raspberry Pi Zero 2 W Rev 1.0" BOARD_LABEL)"
if host_board_field "Raspberry Pi 2 Model B Rev 1.1" BOARD_ID >/dev/null 2>&1; then
    fail "the field accessor must fail for an unsupported board"
fi

# The installer and the deployment probe both quote this list to the operator.
expect_equal "the supported list names four boards" "4" \
    "$(host_supported_boards | wc -l | tr -d ' ')"

if ((failures > 0)); then
    echo "${failures} board profile test(s) failed" >&2
    exit 1
fi

echo "Board profile tests passed"
