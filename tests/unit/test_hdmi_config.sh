#!/bin/bash
# Behavior tests for the installer's HDMI boot-configuration rules.
#
# These two documents decide whether a Pi boots with a picture, so every rule
# is exercised here rather than left to a Pi-only install.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
# shellcheck source=deploy/lib/hdmi-config.sh
source "${ROOT}/deploy/lib/hdmi-config.sh"

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

# ─── Mode validation ─────────────────────────────────────────────────────────

for mode in \
    auto \
    HDMI-A-1:1920x1080@60 \
    HDMI-A-2:1280x720@50 \
    HDMI-A-1:320x200@23 \
    HDMI-A-2:7680x4320@240; do
    host_validate_hdmi_video_mode "${mode}" || fail "rejected a supported mode: ${mode}"
done

for mode in \
    "" \
    Auto \
    auto:1920x1080@60 \
    HDMI-A-3:1920x1080@60 \
    HDMI-A-1:1920x1080 \
    HDMI-A-1:1920x1080@60Hz \
    HDMI-A-1:319x1080@60 \
    HDMI-A-1:7681x1080@60 \
    HDMI-A-1:1920x199@60 \
    HDMI-A-1:1920x4321@60 \
    HDMI-A-1:1920x1080@22 \
    HDMI-A-1:1920x1080@241 \
    HDMI-A-1:0800x1080@60 \
    "HDMI-A-1:1920x1080@60 rm -rf /" \
    'HDMI-A-1:1920x1080@60;reboot'; do
    if host_validate_hdmi_video_mode "${mode}"; then
        fail "accepted an unsupported mode: [${mode}]"
    fi
done

# ─── config.txt managed block ────────────────────────────────────────────────

managed_block=$'# BEGIN OMT Client HDMI configuration\ndtoverlay=vc4-kms-v3d\nmax_framebuffers=2\ndisable_fw_kms_setup=1\n# END OMT Client HDMI configuration'

first_install="$(printf 'dtparam=audio=on\n' | host_hdmi_config_txt)"
expect_equal "first install appends the managed block" \
    $'dtparam=audio=on\n\n'"${managed_block}" "${first_install}"

reinstall="$(printf 'dtparam=audio=on\n\n%s\n' "${managed_block}" | host_hdmi_config_txt)"
expect_equal "reinstall is idempotent" "${first_install}" "${reinstall}"

superseded="$(
    printf 'dtparam=audio=on\n\n# BEGIN OMT Client HDMI configuration\ndtoverlay=stale\n# END OMT Client HDMI configuration\nenable_uart=1\n' |
        host_hdmi_config_txt
)"
expect_equal "a superseded managed block is replaced, not accumulated" \
    $'dtparam=audio=on\n\nenable_uart=1\n\n'"${managed_block}" "${superseded}"

# A crash between staging and rename can leave a block with no end marker.
# Everything inside it is then indistinguishable from the operator's own
# settings, so it is kept rather than silently dropped.
truncated="$(
    printf 'dtparam=audio=on\n# BEGIN OMT Client HDMI configuration\ndtoverlay=stale\nenable_uart=1\n' |
        host_hdmi_config_txt
)"
expect_equal "a truncated managed block keeps unrelated settings" \
    $'dtparam=audio=on\ndtoverlay=stale\nenable_uart=1\n\n'"${managed_block}" "${truncated}"

empty="$(printf '' | host_hdmi_config_txt)"
expect_equal "an empty config still gets the managed block" \
    $'\n'"${managed_block}" "${empty}"

# ─── cmdline.txt video token ─────────────────────────────────────────────────

base="console=serial0,115200 root=/dev/mmcblk0p2 rootwait"

expect_equal "auto leaves an untouched command line alone" \
    "${base}" "$(host_hdmi_cmdline_line "${base}" "" "" "")"

expect_equal "a forced mode is appended once" \
    "${base} video=HDMI-A-1:1920x1080@60D" \
    "$(host_hdmi_cmdline_line "${base}" "" "video=HDMI-A-1:1920x1080@60D" "HDMI-A-1")"

expect_equal "a previously installed token is replaced in place" \
    "${base} video=HDMI-A-1:1280x720@60D" \
    "$(host_hdmi_cmdline_line "${base} video=HDMI-A-1:1920x1080@60D" \
        "video=HDMI-A-1:1920x1080@60D" "video=HDMI-A-1:1280x720@60D" "HDMI-A-1")"

expect_equal "returning to auto removes the previously installed token" \
    "${base}" \
    "$(host_hdmi_cmdline_line "${base} video=HDMI-A-1:1920x1080@60D" \
        "video=HDMI-A-1:1920x1080@60D" "" "")"

expect_equal "a forced mode on the other connector is left in place" \
    "${base} video=HDMI-A-2:1280x720@60D video=HDMI-A-1:1920x1080@60D" \
    "$(host_hdmi_cmdline_line "${base} video=HDMI-A-2:1280x720@60D" \
        "" "video=HDMI-A-1:1920x1080@60D" "HDMI-A-1")"

if host_hdmi_cmdline_line "${base} video=HDMI-A-1:800x600@60" \
        "" "video=HDMI-A-1:1920x1080@60D" "HDMI-A-1" 2>/dev/null; then
    fail "an unmanaged video setting for the target connector was overwritten"
fi

expect_equal "repeated runs of the same forced mode do not duplicate it" \
    "${base} video=HDMI-A-1:1920x1080@60D" \
    "$(host_hdmi_cmdline_line "${base} video=HDMI-A-1:1920x1080@60D" \
        "video=HDMI-A-1:1920x1080@60D" "video=HDMI-A-1:1920x1080@60D" "HDMI-A-1")"

if ((failures > 0)); then
    echo "${failures} HDMI configuration test(s) failed" >&2
    exit 1
fi

echo "HDMI boot-configuration tests passed"
