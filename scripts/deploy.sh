#!/bin/bash
# Upload, verify, recover, and journal-promote a manifest-v2 deployment.

set -euo pipefail
export LC_ALL=C
umask 077

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HOST="${1:-}"
REMOTE_DIR="${2:-/opt/omt-client}"
MANIFEST="${PROJECT_ROOT}/deploy/manifest-v2.txt"
TRANSACTION_HELPER="${PROJECT_ROOT}/deploy/transaction.sh"

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

IFS= read -r manifest_version < "${MANIFEST}" || true
[[ "${manifest_version}" == "version=2" ]] || {
    echo "ERROR: deployment manifest must begin with version=2." >&2
    exit 1
}

ARTIFACT_NAMES=()
declare -A seen_names=()
while IFS= read -r name || [[ -n "${name}" ]]; do
    [[ -n "${name}" && ${#name} -le 240 ]] || {
        echo "ERROR: deployment artifact manifest is invalid." >&2
        exit 1
    }
    if [[ ! "${name}" =~ ^[A-Za-z0-9._/-]+$ || "${name}" == /* || \
          "${name}" == */ || "${name}" == *"//"* || "${name}" == */./* || \
          "${name}" == */../* || "${name}" == "." || "${name}" == ".." || \
          -n "${seen_names[${name}]+x}" ]]; then
        echo "ERROR: deployment artifact manifest is invalid." >&2
        exit 1
    fi
    seen_names["${name}"]=1
    ARTIFACT_NAMES+=("${name}")
    (( ${#ARTIFACT_NAMES[@]} <= 128 )) || {
        echo "ERROR: deployment artifact manifest has too many entries." >&2
        exit 1
    }
done < <(tail -n +2 -- "${MANIFEST}")
(( ${#ARTIFACT_NAMES[@]} > 0 )) || {
    echo "ERROR: deployment artifact manifest is empty." >&2
    exit 1
}
[[ -n "${seen_names[deploy/transaction.sh]+x}" && \
   -n "${seen_names[deploy/manifest-v2.txt]+x}" ]] || {
    echo "ERROR: v2 manifest must include its transaction helper and manifest." >&2
    exit 1
}

declare -A LOCAL_PATHS LOCAL_DIGESTS LOCAL_IDENTITIES
for name in "${ARTIFACT_NAMES[@]}"; do
    path="${PROJECT_ROOT}/${name}"
    if [[ ! -f "${path}" || -L "${path}" ]]; then
        echo "ERROR: required deployment artifact is missing or unsafe: ${path}" >&2
        exit 1
    fi
    LOCAL_PATHS["${name}"]="${path}"
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
REMOTE_STAGE="${REMOTE_DIR}/.deploy-staging/${token}"
cleanup_required=true
cleanup() {
    if [[ "${cleanup_required}" == "true" ]]; then
        ssh "${HOST}" \
            "if [ -d '${REMOTE_STAGE}' ] && [ ! -L '${REMOTE_STAGE}' ]; then find -P '${REMOTE_STAGE}' -xdev -depth -delete; fi" \
            >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

echo "Preparing ${REMOTE_DIR} on ${HOST}..."
ssh -t "${HOST}" \
    "sudo install -d -m 755 -o \"\$(id -u)\" -g \"\$(id -g)\" '${REMOTE_DIR}'"

# Settle journals with the helper and manifest that created them. This must
# happen before v2 can create nested-path state.
ssh "${HOST}" \
    "if [ -x '${REMOTE_DIR}/deploy-transaction.sh' ] && [ -f '${REMOTE_DIR}/deploy-artifacts.txt' ]; then '${REMOTE_DIR}/deploy-transaction.sh' recover '${REMOTE_DIR}' '${REMOTE_DIR}/deploy-artifacts.txt'; fi; if [ -x '${REMOTE_DIR}/deploy/transaction.sh' ]; then '${REMOTE_DIR}/deploy/transaction.sh' recover '${REMOTE_DIR}'; fi"

remote_directories=("${REMOTE_STAGE}")
declare -A seen_directories=()
for name in "${ARTIFACT_NAMES[@]}"; do
    parent="${name%/*}"
    if [[ "${parent}" != "${name}" && -z "${seen_directories[${parent}]+x}" ]]; then
        seen_directories["${parent}"]=1
        remote_directories+=("${REMOTE_STAGE}/${parent}")
    fi
done
ssh "${HOST}" mkdir -p -- "${remote_directories[@]}"

for name in "${ARTIFACT_NAMES[@]}"; do
    local_path="${LOCAL_PATHS[${name}]}"
    staged_path="${REMOTE_STAGE}/${name}"
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
ssh "${HOST}" bash "${REMOTE_STAGE}/deploy/transaction.sh" \
    promote "${REMOTE_DIR}" "${token}" \
    "${REMOTE_STAGE}/deploy/manifest-v2.txt"

cleanup_required=false
ssh -t "${HOST}" \
    "chmod +x '${REMOTE_DIR}/deploy/host/install.sh' '${REMOTE_DIR}/deploy/host/uninstall.sh' '${REMOTE_DIR}/deploy/host/host-diagnostics.sh' '${REMOTE_DIR}/deploy/host/host-reboot.sh' '${REMOTE_DIR}/deploy/transaction.sh' && sudo '${REMOTE_DIR}/deploy/host/install.sh'"
echo "Deployed. Use the authoritative Web UI URL printed by install.sh above."
