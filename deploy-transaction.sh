#!/bin/bash
# Recover or promote the core deployment set under one durable journal.

set -euo pipefail
export LC_ALL=C
umask 077

command_name="${1:-}"
deployment_dir="${2:-}"
token="${3:-}"
manifest_path="${4:-$(dirname -- "$0")/deploy-artifacts.txt}"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

if [[ "${command_name}" != "promote" && "${command_name}" != "recover" ]]; then
    fail "usage: $0 promote <directory> <24-hex-token> [manifest] | recover <directory> [manifest]"
fi
if [[ "${deployment_dir}" == "/" || \
      ! "${deployment_dir}" =~ ^/[A-Za-z0-9._/-]+$ || \
      "${deployment_dir}" == *"//"* || "${deployment_dir}" == */./* || \
      "${deployment_dir}" == */../* || "${deployment_dir}" == */. || \
      "${deployment_dir}" == */.. ]]; then
    fail "deployment directory is not a normalized safe absolute path: ${deployment_dir}"
fi
if [[ ! -d "${deployment_dir}" || -L "${deployment_dir}" ]]; then
    fail "unsafe deployment directory: ${deployment_dir}"
fi
if [[ "${command_name}" == "promote" && ! "${token}" =~ ^[0-9a-f]{24}$ ]]; then
    fail "deployment transaction identifier must contain 24 lowercase hex characters"
fi
if [[ "${command_name}" == "recover" ]]; then
    manifest_path="${3:-$(dirname -- "$0")/deploy-artifacts.txt}"
fi
if [[ ! -f "${manifest_path}" || -L "${manifest_path}" ]]; then
    fail "unsafe or missing deployment artifact manifest: ${manifest_path}"
fi
if (( $(stat -c '%s' -- "${manifest_path}") > 4096 )); then
    fail "deployment artifact manifest exceeds 4096 bytes"
fi

names=()
declare -A seen_names=()
while IFS= read -r name || [[ -n "${name}" ]]; do
    [[ -n "${name}" ]] || continue
    [[ "${name}" =~ ^[A-Za-z0-9._-]+$ ]] || \
        fail "unsafe deployment artifact name: ${name}"
    [[ -z "${seen_names[${name}]:-}" ]] || \
        fail "duplicate deployment artifact name: ${name}"
    seen_names["${name}"]=1
    names+=("${name}")
    (( ${#names[@]} <= 32 )) || fail "deployment artifact manifest has too many entries"
done < "${manifest_path}"
(( ${#names[@]} > 0 )) || fail "deployment artifact manifest is empty"

journal_root="${deployment_dir}/.deploy-transactions"
transaction_committed=false
prepared=""
committed=""

sync_dir() { sync -d "$1"; }

validate_prepared() {
    local journal="$1" name marker_count
    if [[ ! -f "${journal}/ready" || -L "${journal}/ready" ]]; then
        echo "ERROR: unsafe prepared deployment ready marker: ${journal}" >&2
        return 1
    fi
    for name in "${names[@]}"; do
        marker_count=0
        for suffix in present absent; do
            marker="${journal}/${name}.${suffix}"
            if [[ -e "${marker}" || -L "${marker}" ]]; then
                if [[ ! -f "${marker}" || -L "${marker}" ]]; then
                    echo "ERROR: unsafe ${suffix} marker for ${name}" >&2
                    return 1
                fi
                marker_count=$((marker_count + 1))
            fi
        done
        if [[ "${marker_count}" -ne 1 ]]; then
            echo "ERROR: prepared journal lacks one unambiguous marker for ${name}" >&2
            return 1
        fi
        if [[ -e "${journal}/${name}.old" || -L "${journal}/${name}.old" ]]; then
            if [[ ! -f "${journal}/${name}.old" || \
                  -L "${journal}/${name}.old" || \
                  ! -e "${journal}/${name}.present" ]]; then
                echo "ERROR: unsafe deployment backup for ${name}" >&2
                return 1
            fi
        fi
    done
}

recover_pending() {
    local journal name final completed
    [[ -d "${journal_root}" ]] || return 0
    for journal in "${journal_root}"/*.prepared; do
        [[ -e "${journal}" || -L "${journal}" ]] || continue
        if [[ ! -d "${journal}" || -L "${journal}" ]]; then
            echo "ERROR: unsafe prepared deployment journal: ${journal}" >&2
            return 1
        fi
        if [[ ! -e "${journal}/ready" && ! -L "${journal}/ready" ]]; then
            for name in "${names[@]}"; do
                rm -f -- "${journal}/${name}.present" "${journal}/${name}.absent"
            done
            if ! rmdir -- "${journal}"; then
                echo "ERROR: unready deployment journal has unexpected state: ${journal}" >&2
                return 1
            fi
            sync_dir "${journal_root}"
            continue
        fi
        validate_prepared "${journal}"
        for name in "${names[@]}"; do
            final="${deployment_dir}/${name}"
            if [[ -e "${journal}/${name}.old" || -L "${journal}/${name}.old" ]]; then
                rm -f -- "${final}"
                mv -fT -- "${journal}/${name}.old" "${final}"
            elif [[ -e "${journal}/${name}.absent" ]]; then
                rm -f -- "${final}"
            fi
        done
        sync_dir "${deployment_dir}"
        sync_dir "${journal}"
        completed="${journal%.prepared}.committed"
        [[ ! -e "${completed}" && ! -L "${completed}" ]] || {
            echo "ERROR: deployment rollback completion already exists: ${completed}" >&2
            return 1
        }
        mv -fT -- "${journal}" "${completed}"
        sync_dir "${journal_root}"
    done
    for journal in "${journal_root}"/*.committed; do
        [[ -e "${journal}" || -L "${journal}" ]] || continue
        if [[ ! -d "${journal}" || -L "${journal}" ]]; then
            echo "ERROR: unsafe committed deployment journal: ${journal}" >&2
            return 1
        fi
        for name in "${names[@]}"; do
            rm -f -- "${journal}/${name}.old" "${journal}/${name}.present" \
                "${journal}/${name}.absent"
        done
        rm -f -- "${journal}/ready"
        rmdir -- "${journal}"
        sync_dir "${journal_root}"
    done
    rmdir -- "${journal_root}" 2>/dev/null || true
    sync_dir "${deployment_dir}"
}

if [[ -L "${journal_root}" || ( -e "${journal_root}" && ! -d "${journal_root}" ) ]]; then
    fail "unsafe deployment transaction root: ${journal_root}"
fi
exec 9<"${deployment_dir}"
flock -x 9

if [[ "${command_name}" == "recover" ]]; then
    recover_pending
    exit 0
fi

mkdir -p -- "${journal_root}"
sync_dir "${deployment_dir}"
recover_pending
mkdir -p -- "${journal_root}"
prepared="${journal_root}/${token}.prepared"
committed="${journal_root}/${token}.committed"

rollback() {
    local status=$? name final rollback_ok=true
    trap - EXIT HUP INT TERM
    if [[ "${transaction_committed}" != "true" && -d "${prepared}" ]]; then
        for name in "${names[@]}"; do
            final="${deployment_dir}/${name}"
            if [[ -e "${prepared}/${name}.old" || -L "${prepared}/${name}.old" ]]; then
                rm -f -- "${final}" || rollback_ok=false
                mv -fT -- "${prepared}/${name}.old" "${final}" || rollback_ok=false
            elif [[ -e "${prepared}/${name}.absent" ]]; then
                rm -f -- "${final}" || rollback_ok=false
            fi
        done
        if [[ "${rollback_ok}" == "true" ]] && \
           sync_dir "${deployment_dir}" && sync_dir "${prepared}" && \
           mv -fT -- "${prepared}" "${committed}" && sync_dir "${journal_root}"; then
            for name in "${names[@]}"; do
                rm -f -- "${committed}/${name}.old" \
                    "${committed}/${name}.present" \
                    "${committed}/${name}.absent" || true
            done
            rm -f -- "${committed}/ready" || true
            rmdir -- "${committed}" 2>/dev/null || true
            sync_dir "${journal_root}" || true
        fi
    fi
    exit "${status}"
}
trap rollback EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -- "${prepared}"
sync_dir "${journal_root}"
# phase:prepare-directory
for name in "${names[@]}"; do
    final="${deployment_dir}/${name}"
    if [[ -e "${final}" || -L "${final}" ]]; then
        [[ -f "${final}" && ! -L "${final}" ]] || \
            fail "unsafe existing deployment artifact: ${final}"
        : > "${prepared}/${name}.present"
    else
        : > "${prepared}/${name}.absent"
    fi
done
sync_dir "${prepared}"
# phase:prepare-markers
: > "${prepared}/ready"
sync -f "${prepared}/ready"
sync_dir "${prepared}"
sync_dir "${journal_root}"
# phase:ready-fsync
for name in "${names[@]}"; do
    staged="${deployment_dir}/.${name}.upload-${token}"
    [[ -f "${staged}" && ! -L "${staged}" ]] || \
        fail "unsafe staged deployment artifact: ${staged}"
    sync -f "${staged}"
    if [[ -e "${prepared}/${name}.present" ]]; then
        mv -fT -- "${deployment_dir}/${name}" "${prepared}/${name}.old"
    fi
    sync_dir "${deployment_dir}"
    sync_dir "${prepared}"
    # phase:backup
    mv -fT -- "${staged}" "${deployment_dir}/${name}"
    sync -f "${deployment_dir}/${name}"
    sync_dir "${deployment_dir}"
    # phase:promote
done
mv -fT -- "${prepared}" "${committed}"
sync_dir "${journal_root}"
transaction_committed=true
# phase:committed
for name in "${names[@]}"; do
    rm -f -- "${committed}/${name}.old" "${committed}/${name}.present" \
        "${committed}/${name}.absent"
done
rm -f -- "${committed}/ready"
rmdir -- "${committed}"
rmdir -- "${journal_root}" 2>/dev/null || true
sync_dir "${deployment_dir}"
trap - EXIT HUP INT TERM

# Deployment support files are not part of the runtime artifact transaction.
# Publish them only after the five-file set has committed, so an older installed
# recovery helper can always understand any journal left during an upgrade.
staged_helper="${deployment_dir}/.deploy-transaction.sh.upload-${token}"
staged_manifest="${deployment_dir}/.deploy-artifacts.txt.upload-${token}"
if [[ "$0" == "${staged_helper}" && "${manifest_path}" == "${staged_manifest}" ]]; then
    chmod 0755 "${staged_helper}"
    chmod 0644 "${staged_manifest}"
    mv -fT -- "${staged_manifest}" "${deployment_dir}/deploy-artifacts.txt"
    mv -fT -- "${staged_helper}" "${deployment_dir}/deploy-transaction.sh"
    sync -f "${deployment_dir}/deploy-artifacts.txt"
    sync -f "${deployment_dir}/deploy-transaction.sh"
    sync_dir "${deployment_dir}"
fi
