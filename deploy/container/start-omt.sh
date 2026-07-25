#!/bin/bash
set -euo pipefail

export LC_ALL=C
umask 077

OMT_CONFIG_DIR="${OMT_CONFIG_DIR:-/etc/omt}"
OMT_SOURCE_TARGET_FILE="${OMT_SOURCE_TARGET_FILE:-${OMT_CONFIG_DIR}/source_target.json}"
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

target="$(
    python3 - "${OMT_SOURCE_TARGET_FILE}" <<'PY'
import json
import os
import stat
import sys

path = sys.argv[1]
before = os.lstat(path)
if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode) or before.st_size > 1024:
    raise SystemExit("unsafe OMT source target")
flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
fd = os.open(path, flags)
try:
    opened = os.fstat(fd)
    if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
        raise SystemExit("OMT source target changed while opening")
    raw = os.read(fd, 1025)
finally:
    os.close(fd)
if len(raw) > 1024:
    raise SystemExit("OMT source target is oversized")
document = json.loads(raw)
if document.get("schema") != 1:
    raise SystemExit("unsupported OMT source target schema")
if document.get("kind") == "discovered" and set(document) == {"schema", "kind", "name"}:
    value = document["name"]
elif document.get("kind") == "direct" and set(document) == {"schema", "kind", "uri"}:
    value = document["uri"]
else:
    raise SystemExit("invalid OMT source target")
if not isinstance(value, str) or not value:
    raise SystemExit("empty OMT source target")
print(value)
PY
)"

mkdir -p "$(dirname -- "${OMT_PLAYBACK_STATUS_FILE}")"
exec "${OMT_RECEIVER_COMMAND}" play \
    --target "${target}" \
    --connector "${OMT_HDMI_CONNECTOR}" \
    --status-file "${OMT_PLAYBACK_STATUS_FILE}"
