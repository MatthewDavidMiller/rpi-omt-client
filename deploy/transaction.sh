#!/bin/bash
# Recover or promote a manifest-v3 deployment under one durable journal.

set -euo pipefail
export LC_ALL=C
umask 077

command_name="${1:-}"
deployment_dir="${2:-}"
token="${3:-}"
manifest_path="${4:-}"

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

safe_absolute_directory() {
    local path="$1"
    [[ "${path}" != "/" && "${path}" =~ ^/[A-Za-z0-9._/-]+$ ]] &&
        [[ "${path}" != *"//"* && "${path}" != */./* && "${path}" != */../* ]] &&
        [[ "${path}" != */. && "${path}" != */.. ]]
}

safe_relative_path() {
    local path="$1"
    [[ -n "${path}" && ${#path} -le 240 ]] &&
        [[ "${path}" =~ ^[A-Za-z0-9._/-]+$ ]] &&
        [[ "${path}" != /* && "${path}" != */ && "${path}" != *"//"* ]] &&
        [[ "${path}" != "." && "${path}" != ".." ]] &&
        [[ "${path}" != ./* && "${path}" != ../* ]] &&
        [[ "${path}" != */./* && "${path}" != */../* ]] &&
        [[ "${path}" != */. && "${path}" != */.. ]]
}

sync_dir() {
    sync -d "$1"
}

remove_tree() {
    local path="$1"
    [[ "${path}" == "${journal_root}/"* || "${path}" == "${staging_root}/"* ]] ||
        fail "refusing to remove an out-of-scope transaction path: ${path}"
    [[ -d "${path}" && ! -L "${path}" ]] || return 0
    find -P "${path}" -xdev -depth -delete
}

assert_safe_ancestors() {
    local root="$1"
    local relative="$2"
    local current="${root}"
    local component
    IFS='/' read -r -a components <<< "${relative}"
    for component in "${components[@]:0:${#components[@]}-1}"; do
        current="${current}/${component}"
        if [[ -L "${current}" || ( -e "${current}" && ! -d "${current}" ) ]]; then
            fail "unsafe ancestor for deployment path ${relative}: ${current}"
        fi
    done
}

ensure_parent_directories() {
    local root="$1"
    local relative="$2"
    local parent
    assert_safe_ancestors "${root}" "${relative}"
    parent="${root}/$(dirname -- "${relative}")"
    if [[ "${parent}" != "${root}/." ]]; then
        mkdir -p -- "${parent}"
    fi
}

load_manifest() {
    local source="$1"
    local line
    names=()
    declare -gA seen_names=()
    [[ -f "${source}" && ! -L "${source}" ]] ||
        fail "unsafe or missing deployment manifest: ${source}"
    (( $(stat -c '%s' -- "${source}") <= 32768 )) ||
        fail "deployment manifest exceeds 32768 bytes"
    IFS= read -r line < "${source}" || true
    [[ "${line}" == "version=3" ]] ||
        fail "deployment manifest must begin with version=3"
    while IFS= read -r line || [[ -n "${line}" ]]; do
        [[ -n "${line}" ]] || fail "deployment manifest contains an empty path"
        safe_relative_path "${line}" ||
            fail "unsafe deployment artifact path: ${line}"
        [[ -z "${seen_names[${line}]+x}" ]] ||
            fail "duplicate deployment artifact path: ${line}"
        seen_names["${line}"]=1
        names+=("${line}")
        (( ${#names[@]} <= 128 )) ||
            fail "deployment manifest has too many entries"
    done < <(tail -n +2 -- "${source}")
    (( ${#names[@]} > 0 )) || fail "deployment manifest is empty"
}

cleanup_empty_parents() {
    local relative="$1"
    local parent
    parent="$(dirname -- "${relative}")"
    while [[ "${parent}" != "." ]]; do
        rmdir -- "${deployment_dir}/${parent}" 2>/dev/null || break
        parent="$(dirname -- "${parent}")"
    done
}

validate_journal() {
    local journal="$1"
    [[ -d "${journal}" && ! -L "${journal}" ]] ||
        fail "unsafe deployment journal: ${journal}"
    [[ -f "${journal}/manifest-v3.txt" && ! -L "${journal}/manifest-v3.txt" ]] ||
        fail "deployment journal has no safe transaction manifest"
    [[ -f "${journal}/state.tsv" && ! -L "${journal}/state.tsv" ]] ||
        fail "deployment journal has no safe state record"
    [[ -f "${journal}/ready" && ! -L "${journal}/ready" ]] ||
        fail "deployment journal has no safe ready marker"
    load_manifest "${journal}/manifest-v3.txt"
    local expected_lines="${#names[@]}"
    (( $(wc -l < "${journal}/state.tsv") == expected_lines )) ||
        fail "deployment journal state count does not match its manifest"
}

rollback_prepared() {
    local journal="$1"
    local relative prior final backup recorded
    validate_journal "${journal}"
    declare -A prior_states=()
    while IFS=$'\t' read -r relative prior recorded; do
        [[ -z "${recorded}" && -n "${relative}" ]] ||
            fail "malformed deployment journal state"
        [[ -n "${seen_names[${relative}]+x}" ]] ||
            fail "journal state contains a path outside its manifest"
        [[ "${prior}" == "present" || "${prior}" == "absent" ]] ||
            fail "journal state has an invalid prior state"
        [[ -z "${prior_states[${relative}]+x}" ]] ||
            fail "journal state contains a duplicate path"
        prior_states["${relative}"]="${prior}"
    done < "${journal}/state.tsv"
    for relative in "${names[@]}"; do
        final="${deployment_dir}/${relative}"
        backup="${journal}/backup/${relative}"
        if [[ "${prior_states[${relative}]}" == "present" ]]; then
            if [[ -e "${backup}" || -L "${backup}" ]]; then
                [[ -f "${backup}" && ! -L "${backup}" ]] ||
                    fail "unsafe deployment backup: ${backup}"
                rm -f -- "${final}"
                ensure_parent_directories "${deployment_dir}" "${relative}"
                mv -fT -- "${backup}" "${final}"
            fi
        else
            rm -f -- "${final}"
            cleanup_empty_parents "${relative}"
        fi
    done
    sync_dir "${deployment_dir}"
}

recover_pending() {
    local journal completed
    [[ -d "${journal_root}" ]] || return 0
    [[ ! -L "${journal_root}" ]] || fail "unsafe deployment journal root"
    for journal in "${journal_root}"/*.prepared; do
        [[ -e "${journal}" || -L "${journal}" ]] || continue
        if [[ ! -e "${journal}/ready" && ! -L "${journal}/ready" ]]; then
            [[ -d "${journal}" && ! -L "${journal}" ]] ||
                fail "unsafe incomplete deployment journal"
            remove_tree "${journal}"
            sync_dir "${journal_root}"
            continue
        fi
        rollback_prepared "${journal}"
        completed="${journal%.prepared}.rolled-back"
        [[ ! -e "${completed}" && ! -L "${completed}" ]] ||
            fail "rollback completion already exists: ${completed}"
        mv -fT -- "${journal}" "${completed}"
        sync_dir "${journal_root}"
    done
    for journal in "${journal_root}"/*.committed \
                   "${journal_root}"/*.rolled-back; do
        [[ -e "${journal}" || -L "${journal}" ]] || continue
        [[ -d "${journal}" && ! -L "${journal}" ]] ||
            fail "unsafe completed deployment journal: ${journal}"
        remove_tree "${journal}"
        sync_dir "${journal_root}"
    done
    rmdir -- "${journal_root}" 2>/dev/null || true
    sync_dir "${deployment_dir}"
}

if [[ "${command_name}" != "promote" && "${command_name}" != "recover" ]]; then
    fail "usage: $0 promote <directory> <24-hex-token> <manifest> | recover <directory>"
fi
safe_absolute_directory "${deployment_dir}" ||
    fail "deployment directory is not a normalized safe absolute path"
[[ -d "${deployment_dir}" && ! -L "${deployment_dir}" ]] ||
    fail "unsafe deployment directory: ${deployment_dir}"
if [[ "${command_name}" == "promote" ]]; then
    [[ "${token}" =~ ^[0-9a-f]{24}$ ]] ||
        fail "deployment transaction identifier must contain 24 lowercase hex characters"
    [[ -n "${manifest_path}" ]] || fail "promote requires the staged v3 manifest"
fi

journal_root="${deployment_dir}/.deploy-transactions"
staging_root="${deployment_dir}/.deploy-staging"
if [[ -L "${staging_root}" || ( -e "${staging_root}" && ! -d "${staging_root}" ) ]]; then
    fail "unsafe deployment staging root"
fi
exec 9<"${deployment_dir}"
flock -x 9

if [[ "${command_name}" == "recover" ]]; then
    recover_pending
    exit 0
fi

staging="${staging_root}/${token}"
[[ -d "${staging}" && ! -L "${staging}" ]] ||
    fail "unsafe or missing token-specific staging directory"
[[ "${manifest_path}" == "${staging}/deploy/manifest-v3.txt" ]] ||
    fail "promotion must use the manifest inside its token-specific stage"
load_manifest "${manifest_path}"
for relative in "${names[@]}"; do
    assert_safe_ancestors "${staging}" "${relative}"
    staged="${staging}/${relative}"
    [[ -f "${staged}" && ! -L "${staged}" ]] ||
        fail "unsafe or missing staged deployment artifact: ${relative}"
    assert_safe_ancestors "${deployment_dir}" "${relative}"
    final="${deployment_dir}/${relative}"
    if [[ -e "${final}" || -L "${final}" ]]; then
        [[ -f "${final}" && ! -L "${final}" ]] ||
            fail "unsafe existing deployment artifact: ${relative}"
    fi
done

if [[ -L "${journal_root}" || ( -e "${journal_root}" && ! -d "${journal_root}" ) ]]; then
    fail "unsafe deployment transaction root"
fi
mkdir -p -- "${journal_root}"
sync_dir "${deployment_dir}"
recover_pending
mkdir -p -- "${journal_root}"

prepared="${journal_root}/${token}.prepared"
committed="${journal_root}/${token}.committed"
transaction_committed=false

rollback() {
    local status=$?
    trap - EXIT HUP INT TERM
    if [[ "${transaction_committed}" != "true" && -d "${prepared}" ]]; then
        rollback_prepared "${prepared}" || true
    fi
    exit "${status}"
}
trap rollback EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -- "${prepared}"
mkdir -- "${prepared}/backup"
cp -- "${manifest_path}" "${prepared}/manifest-v3.txt"
cmp -s -- "${manifest_path}" "${prepared}/manifest-v3.txt" ||
    fail "staged manifest changed while journaling"
: > "${prepared}/state.tsv"
sync_dir "${journal_root}"
# phase:prepare-directory

for relative in "${names[@]}"; do
    final="${deployment_dir}/${relative}"
    if [[ -e "${final}" || -L "${final}" ]]; then
        printf '%s\tpresent\n' "${relative}" >> "${prepared}/state.tsv"
    else
        printf '%s\tabsent\n' "${relative}" >> "${prepared}/state.tsv"
    fi
done
sync -f "${prepared}/manifest-v3.txt"
sync -f "${prepared}/state.tsv"
: > "${prepared}/ready"
sync -f "${prepared}/ready"
sync_dir "${prepared}"
# phase:journal

for relative in "${names[@]}"; do
    final="${deployment_dir}/${relative}"
    staged="${staging}/${relative}"
    backup="${prepared}/backup/${relative}"
    ensure_parent_directories "${prepared}/backup" "${relative}"
    ensure_parent_directories "${deployment_dir}" "${relative}"
    if [[ -e "${final}" ]]; then
        mv -fT -- "${final}" "${backup}"
        sync_dir "$(dirname -- "${backup}")"
    fi
    sync_dir "$(dirname -- "${final}")"
    # phase:backup
    mv -fT -- "${staged}" "${final}"
    sync -f "${final}"
    sync_dir "$(dirname -- "${final}")"
    # phase:promotion
done

mv -fT -- "${prepared}" "${committed}"
sync_dir "${journal_root}"
transaction_committed=true
# phase:commit

remove_tree "${committed}"
rmdir -- "${journal_root}" 2>/dev/null || true
remove_tree "${staging}"
rmdir -- "${staging_root}" 2>/dev/null || true
sync_dir "${deployment_dir}"
# phase:cleanup
trap - EXIT HUP INT TERM
