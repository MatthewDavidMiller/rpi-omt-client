#!/bin/bash
set -euo pipefail

export LC_ALL=C
umask 077

OMT_CONFIG_DIR="${OMT_CONFIG_DIR:-/etc/omt}"
OMT_SOURCE_TARGET_FILE="${OMT_SOURCE_TARGET_FILE:-${OMT_CONFIG_DIR}/source_target.json}"
OMT_VIDEO_CEILING_FILE="${OMT_VIDEO_CEILING_FILE:-${OMT_CONFIG_DIR}/video_ceiling.json}"
OMT_VIDEO_CEILING="${OMT_VIDEO_CEILING:-1920x1080@60}"
OMT_RUNTIME_DIR="${OMT_RUNTIME_DIR:-${OMT_CONFIG_DIR}/run}"
OMT_PLAYBACK_STATUS_FILE="${OMT_PLAYBACK_STATUS_FILE:-${OMT_RUNTIME_DIR}/playback-status.json}"
OMT_RECEIVER_COMMAND="${OMT_RECEIVER_COMMAND:-/usr/local/bin/omt-receiver}"
OMT_HDMI_CONNECTOR="${OMT_HDMI_CONNECTOR:-auto}"
OMT_STORAGE_PATH="${OMT_STORAGE_PATH:-${OMT_CONFIG_DIR}/omt}"
export OMT_STORAGE_PATH

if [[ ! "${OMT_HDMI_CONNECTOR}" =~ ^(auto|HDMI-A-1|HDMI-A-2)$ ]]; then
    echo "Invalid OMT_HDMI_CONNECTOR: ${OMT_HDMI_CONNECTOR}" >&2
    exit 2
fi

# One shared reader with the Rust Web service: schema, size, symlink, and
# validation rules live in omt-web rather than a second copy here.
target="$(omt-web play-target "${OMT_SOURCE_TARGET_FILE}")"

# The board's decode ceiling, or the operator's override of it. Resolved by the
# same binary for the same reason: the ceiling grammar and its bounds live in
# one place rather than being restated by whoever builds the argument vector.
ceiling="$(omt-web video-ceiling \
    "${OMT_VIDEO_CEILING_FILE}" "${OMT_VIDEO_CEILING}")"

mkdir -p "$(dirname -- "${OMT_PLAYBACK_STATUS_FILE}")"
exec "${OMT_RECEIVER_COMMAND}" play \
    --target "${target}" \
    --connector "${OMT_HDMI_CONNECTOR}" \
    --status-file "${OMT_PLAYBACK_STATUS_FILE}" \
    --video-ceiling "${ceiling}"
