#!/bin/bash
# Upload, verify, and journal-promote the complete CLI deployment artifact set.

set -euo pipefail
export LC_ALL=C
umask 077

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HOST="${1:-}"
REMOTE_DIR="${2:-/opt/omt-client}"
MANIFEST="${PROJECT_ROOT}/deploy-artifacts.txt"
TRANSACTION_HELPER="${PROJECT_ROOT}/deploy-transaction.sh"

if [[ -z "${HOST}" || "${HOST}" == -* || \
      ! "${HOST}" =~ ^([A-Za-z0-9._-]+@)?(\[[0-9A-Fa-f:]+\]|[A-Za-z0-9][A-Za-z0-9.-]*)$ ]]; then
    echo "ERROR: HOST must be a safe SSH hostname/IP with an optional username." >&2
    exit 2
fi
if [[ "${REMOTE_DIR}" == "/" || ! "${REMOTE_DIR}" =~ ^/[A-Za-z0-9._/-]+$ || \
      "${REMOTE_DIR}" == *"//"* || "${REMOTE_DIR}" == */./* || \
      "${REMOTE_DIR}" == */../* || "${REMOTE_DIR}" == */. || \
      "${REMOTE_DIR}" == */.. ]]; then
    echo "ERROR: remote directory is not a normalized safe absolute path." >&2
    exit 2
fi
for support_file in "${MANIFEST}" "${TRANSACTION_HELPER}"; do
    if [[ ! -f "${support_file}" || -L "${support_file}" ]]; then
        echo "ERROR: required deployment support file is missing or unsafe: ${support_file}" >&2
        exit 1
    fi
done

ARTIFACT_NAMES=()
declare -A seen_names=()
while IFS= read -r name || [[ -n "${name}" ]]; do
    [[ -n "${name}" ]] || continue
    if [[ ! "${name}" =~ ^[A-Za-z0-9._-]+$ || -n "${seen_names[${name}]:-}" ]]; then
        echo "ERROR: deployment artifact manifest is invalid." >&2
        exit 1
    fi
    seen_names["${name}"]=1
    ARTIFACT_NAMES+=("${name}")
done < "${MANIFEST}"
(( ${#ARTIFACT_NAMES[@]} > 0 )) || {
    echo "ERROR: deployment artifact manifest is empty." >&2
    exit 1
}

UPLOAD_NAMES=("${ARTIFACT_NAMES[@]}" deploy-transaction.sh deploy-artifacts.txt)
declare -A LOCAL_PATHS LOCAL_DIGESTS LOCAL_IDENTITIES
for name in "${ARTIFACT_NAMES[@]}"; do
    LOCAL_PATHS["${name}"]="${PROJECT_ROOT}/${name}"
done
LOCAL_PATHS[deploy-transaction.sh]="${TRANSACTION_HELPER}"
LOCAL_PATHS[deploy-artifacts.txt]="${MANIFEST}"
for name in "${UPLOAD_NAMES[@]}"; do
    path="${LOCAL_PATHS[${name}]}"
    if [[ ! -f "${path}" || -L "${path}" ]]; then
        echo "ERROR: required deployment artifact is missing or unsafe: ${path}" >&2
        exit 1
    fi
    LOCAL_IDENTITIES["${name}"]="$(stat -c '%d:%i:%s:%y:%z' -- "${path}")"
    LOCAL_DIGESTS["${name}"]="$(sha256sum -- "${path}" | awk '{print $1}')"
done

echo "Checking target architecture on ${HOST}..."
remote_architecture="$(ssh "${HOST}" uname -m)"
if [[ "${remote_architecture}" != "aarch64" ]]; then
    echo "ERROR: remote host must report aarch64; detected ${remote_architecture:-unknown}." >&2
    exit 1
fi

token="$(od -An -N12 -tx1 /dev/urandom | tr -d '[:space:]')"
[[ "${token}" =~ ^[0-9a-f]{24}$ ]] || {
    echo "ERROR: unable to generate a deployment transaction identifier." >&2
    exit 1
}
cleanup_required=true
cleanup() {
    if [[ "${cleanup_required}" == "true" ]]; then
        staged=()
        for name in "${UPLOAD_NAMES[@]}"; do
            staged+=("${REMOTE_DIR}/.${name}.upload-${token}")
        done
        ssh "${HOST}" rm -f -- "${staged[@]}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

echo "Preparing ${REMOTE_DIR} on ${HOST}..."
ssh -t "${HOST}" \
    "sudo install -d -m 755 -o \"\$(id -u)\" -g \"\$(id -g)\" '${REMOTE_DIR}'"

for name in "${UPLOAD_NAMES[@]}"; do
    local_path="${LOCAL_PATHS[${name}]}"
    staged_path="${REMOTE_DIR}/.${name}.upload-${token}"
    echo "Uploading ${name}..."
    scp "${local_path}" "${HOST}:${staged_path}"
    if [[ "$(stat -c '%d:%i:%s:%y:%z' -- "${local_path}")" != \
          "${LOCAL_IDENTITIES[${name}]}" ]]; then
        echo "ERROR: local deployment artifact changed during upload: ${name}." >&2
        exit 1
    fi
    remote_digest="$(ssh "${HOST}" sha256sum -- "${staged_path}" | awk '{print $1}')"
    if [[ ! "${remote_digest}" =~ ^[0-9a-f]{64}$ || \
          "${remote_digest}" != "${LOCAL_DIGESTS[${name}]}" ]]; then
        echo "ERROR: SHA-256 mismatch after uploading ${name}." >&2
        exit 1
    fi
done

echo "Promoting verified deployment set..."
ssh "${HOST}" bash "${REMOTE_DIR}/.deploy-transaction.sh.upload-${token}" \
    promote "${REMOTE_DIR}" "${token}" \
    "${REMOTE_DIR}/.deploy-artifacts.txt.upload-${token}"

cleanup_required=false
ssh -t "${HOST}" \
    "chmod +x '${REMOTE_DIR}/install.sh' '${REMOTE_DIR}/uninstall.sh' '${REMOTE_DIR}/host-debug.sh' '${REMOTE_DIR}/host-reboot.sh' && sudo '${REMOTE_DIR}/install.sh'"
echo "Deployed. Use the authoritative Web UI URL printed by install.sh above."
