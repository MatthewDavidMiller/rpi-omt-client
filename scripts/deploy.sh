#!/bin/bash
# Upload, verify, recover, and journal-promote a manifest-v3 deployment.

set -euo pipefail
export LC_ALL=C
umask 077

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
HOST="${1:-}"
REMOTE_DIR="${2:-/opt/omt-client}"
MANIFEST="${PROJECT_ROOT}/deploy/manifest-v3.txt"
TRANSACTION_HELPER="${PROJECT_ROOT}/deploy/transaction.sh"
BOOTSTRAP_SCRIPT="${PROJECT_ROOT}/deploy/host/bootstrap.sh"
# Kept in step with deploy/host/install.sh, which refuses any other series.
SUPPORTED_ALPINE_SERIES=3.24
# The supported-board table, shared with the installer rather than restated, so
# this path and the appliance can never disagree about what will install.
# shellcheck source=deploy/lib/board-profile.sh
source "${PROJECT_ROOT}/deploy/lib/board-profile.sh"

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
[[ "${manifest_version}" == "version=3" ]] || {
    echo "ERROR: deployment manifest must begin with version=3." >&2
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
   -n "${seen_names[deploy/manifest-v3.txt]+x}" ]] || {
    echo "ERROR: v3 manifest must include its transaction helper and manifest." >&2
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

echo "Checking the Alpine Raspberry Pi target on ${HOST}..."
remote_platform="$(ssh "${HOST}" 'uname -m; . /etc/os-release; printf "%s\n" "$ID"; cat /etc/alpine-release; tr -d "\000" < /proc/device-tree/model; printf "\n"')"
mapfile -t platform_lines <<< "${remote_platform}"
if [[ "${platform_lines[0]:-}" != "aarch64" || \
      "${platform_lines[1]:-}" != "alpine" || \
      "${platform_lines[2]:-}" != "${SUPPORTED_ALPINE_SERIES}".* ]] || \
   ! host_board_profile "${platform_lines[3]:-}" >/dev/null; then
    echo "ERROR: remote host must run Alpine Linux ${SUPPORTED_ALPINE_SERIES} aarch64 on one of:" >&2
    host_supported_boards | sed 's/^/  - /' >&2
    echo "Detected: ${platform_lines[3]:-unknown}" >&2
    exit 1
fi
echo "Deploying to ${platform_lines[3]}."

# A stock Alpine image has neither bash nor sudo, yet the transaction helper and
# the installer are both bash scripts run through sudo. Probe for the pieces and
# bootstrap them before anything downstream assumes they exist.
remote_capabilities="$(ssh "${HOST}" '
    id -u
    for tool in bash sudo doas; do
        if command -v "$tool" >/dev/null 2>&1; then echo yes; else echo no; fi
    done')"
mapfile -t capability_lines <<< "${remote_capabilities}"
REMOTE_UID="${capability_lines[0]:-}"
HAS_BASH="${capability_lines[1]:-no}"
HAS_SUDO="${capability_lines[2]:-no}"
HAS_DOAS="${capability_lines[3]:-no}"
[[ "${REMOTE_UID}" =~ ^[0-9]+$ ]] || {
    echo "ERROR: could not determine the remote user id." >&2
    exit 1
}

# Prefer no escalation at all when the deploy account is already root; sudo is
# only the fallback, and doas covers a host bootstrapped by hand.
if [[ "${REMOTE_UID}" == 0 ]]; then
    ESCALATE=""
elif [[ "${HAS_SUDO}" == yes ]]; then
    ESCALATE="sudo"
elif [[ "${HAS_DOAS}" == yes ]]; then
    ESCALATE="doas"
else
    echo "ERROR: ${HOST} has no way to become root: no sudo, no doas, and the" >&2
    echo "deploy account is not root. Alpine ships neither by default." >&2
    echo "Fix by running the bootstrap once as root on the Pi:" >&2
    echo "  su -c '/bin/sh /tmp/bootstrap.sh'   # after copying deploy/host/bootstrap.sh" >&2
    echo "or re-run this deploy against the root account: make deploy HOST=root@<ip>" >&2
    exit 1
fi

if [[ "${HAS_BASH}" != yes || "${HAS_SUDO}" != yes ]]; then
    echo "Bootstrapping bash and sudo on ${HOST}..."
    if [[ ! -f "${BOOTSTRAP_SCRIPT}" || -L "${BOOTSTRAP_SCRIPT}" ]]; then
        echo "ERROR: missing or unsafe bootstrap script: ${BOOTSTRAP_SCRIPT}" >&2
        exit 1
    fi
    # /bin/sh explicitly: this is the one script that must run before bash does.
    remote_bootstrap="$(ssh "${HOST}" 'mktemp /tmp/omt-bootstrap.XXXXXX')"
    [[ "${remote_bootstrap}" =~ ^/tmp/omt-bootstrap\.[A-Za-z0-9]+$ ]] || {
        echo "ERROR: could not stage the bootstrap script on ${HOST}." >&2
        exit 1
    }
    scp "${BOOTSTRAP_SCRIPT}" "${HOST}:${remote_bootstrap}"
    ssh -t "${HOST}" "${ESCALATE} /bin/sh '${remote_bootstrap}'; rc=\$?; rm -f -- '${remote_bootstrap}'; exit \$rc"
    # sudo only exists from here on, so an account that had to use doas can stop.
    [[ "${REMOTE_UID}" == 0 ]] || ESCALATE="sudo"
fi

token="$(od -An -N12 -tx1 /dev/urandom | tr -d '[:space:]')"
[[ "${token}" =~ ^[0-9a-f]{24}$ ]] || {
    echo "ERROR: unable to generate a deployment transaction identifier." >&2
    exit 1
}
REMOTE_STAGE="${REMOTE_DIR}/.deploy-staging/${token}"
REMOTE_STAGING_ROOT="${REMOTE_DIR}/.deploy-staging"
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
    "${ESCALATE} install -d -m 755 -o \"\$(id -u)\" -g \"\$(id -g)\" '${REMOTE_DIR}'"

# Settle journals with the helper and manifest that created them. This must
# happen before v3 can create nested-path state.
ssh "${HOST}" \
    "if [ -x '${REMOTE_DIR}/deploy-transaction.sh' ] && [ -f '${REMOTE_DIR}/deploy-artifacts.txt' ]; then '${REMOTE_DIR}/deploy-transaction.sh' recover '${REMOTE_DIR}' '${REMOTE_DIR}/deploy-artifacts.txt'; fi; if [ -x '${REMOTE_DIR}/deploy/transaction.sh' ]; then '${REMOTE_DIR}/deploy/transaction.sh' recover '${REMOTE_DIR}'; fi"

ssh "${HOST}" \
    "if [ -L '${REMOTE_STAGING_ROOT}' ] || { [ -e '${REMOTE_STAGING_ROOT}' ] && [ ! -d '${REMOTE_STAGING_ROOT}' ]; }; then exit 14; fi; install -d -m 700 -- '${REMOTE_STAGING_ROOT}'; mkdir -- '${REMOTE_STAGE}'"

remote_directories=()
declare -A seen_directories=()
for name in "${ARTIFACT_NAMES[@]}"; do
    parent="${name%/*}"
    if [[ "${parent}" != "${name}" && -z "${seen_directories[${parent}]+x}" ]]; then
        seen_directories["${parent}"]=1
        remote_directories+=("${REMOTE_STAGE}/${parent}")
    fi
done
if (( ${#remote_directories[@]} > 0 )); then
    ssh "${HOST}" mkdir -p -- "${remote_directories[@]}"
fi

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
    "${REMOTE_STAGE}/deploy/manifest-v3.txt"

cleanup_required=false
ssh -t "${HOST}" \
    "chmod +x '${REMOTE_DIR}/deploy/host/bootstrap.sh' '${REMOTE_DIR}/deploy/host/install.sh' '${REMOTE_DIR}/deploy/host/uninstall.sh' '${REMOTE_DIR}/deploy/host/host-diagnostics.sh' '${REMOTE_DIR}/deploy/host/host-event-watcher.sh' '${REMOTE_DIR}/deploy/host/host-reboot.sh' '${REMOTE_DIR}/deploy/transaction.sh' && ${ESCALATE} '${REMOTE_DIR}/deploy/host/install.sh'"
echo "Deployed. Use the authoritative Web UI URL printed by install.sh above."
