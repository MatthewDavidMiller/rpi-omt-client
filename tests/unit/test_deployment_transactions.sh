#!/bin/bash
# Behavioral tests for manifest-v3 nested deployment promotion and recovery.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TRANSACTION="${PROJECT_ROOT}/deploy/transaction.sh"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "${TEST_DIR}"' EXIT

write_manifest() {
    local path="$1"
    mkdir -p "$(dirname "${path}")"
    printf 'version=3\nnested/a.txt\nb.txt\n' > "${path}"
}

prepare_generation() {
    local root="$1"
    local token="$2"
    local generation="$3"
    local stage="${root}/.deploy-staging/${token}"
    mkdir -p "${stage}/nested"
    write_manifest "${stage}/manifest-v3.txt"
    # The transaction requires the manifest path to be the v3 production path.
    mkdir -p "${stage}/deploy"
    cp "${stage}/manifest-v3.txt" "${stage}/deploy/manifest-v3.txt"
    printf '%s-a\n' "${generation}" > "${stage}/nested/a.txt"
    printf '%s-b\n' "${generation}" > "${stage}/b.txt"
}

promote() {
    local helper="$1"
    local root="$2"
    local token="$3"
    bash "${helper}" promote "${root}" "${token}" \
        "${root}/.deploy-staging/${token}/deploy/manifest-v3.txt"
}

# Custom fixtures omit the production helper paths but still exercise the v3
# grammar, nested staging, journal-owned manifest, and rollback behavior.
root="${TEST_DIR}/success"
token="0123456789abcdef01234567"
mkdir -p "${root}"
prepare_generation "${root}" "${token}" "new"
promote "${TRANSACTION}" "${root}" "${token}"
[[ "$(< "${root}/nested/a.txt")" == "new-a" ]]
[[ "$(< "${root}/b.txt")" == "new-b" ]]
[[ ! -d "${root}/.deploy-staging" ]]
[[ ! -d "${root}/.deploy-transactions" ]]

# Missing staged files fail before any existing generation is changed.
missing_root="${TEST_DIR}/missing"
mkdir -p "${missing_root}/nested"
printf 'old-a\n' > "${missing_root}/nested/a.txt"
printf 'old-b\n' > "${missing_root}/b.txt"
missing_token="111111111111111111111111"
prepare_generation "${missing_root}" "${missing_token}" "new"
rm "${missing_root}/.deploy-staging/${missing_token}/b.txt"
if promote "${TRANSACTION}" "${missing_root}" "${missing_token}" >/dev/null 2>&1; then
    echo "FAIL: missing staged artifact was accepted" >&2
    exit 1
fi
[[ "$(< "${missing_root}/nested/a.txt")" == "old-a" ]]
[[ "$(< "${missing_root}/b.txt")" == "old-b" ]]

fault_case() {
    local phase="$1"
    local expected="$2"
    local token="$3"
    local case_root="${TEST_DIR}/fault-${phase}"
    local helper="${TEST_DIR}/transaction-${phase}.sh"
    mkdir -p "${case_root}/nested"
    printf 'old-a\n' > "${case_root}/nested/a.txt"
    printf 'old-b\n' > "${case_root}/b.txt"
    prepare_generation "${case_root}" "${token}" "new"
    awk -v marker="# phase:${phase}" '
        { print }
        index($0, marker) { print "exit 97" }
    ' "${TRANSACTION}" > "${helper}"
    chmod +x "${helper}"
    if promote "${helper}" "${case_root}" "${token}" >/dev/null 2>&1; then
        echo "FAIL: injected ${phase} fault did not stop promotion" >&2
        exit 1
    fi
    bash "${TRANSACTION}" recover "${case_root}"
    [[ "$(< "${case_root}/nested/a.txt")" == "${expected}-a" ]]
    [[ "$(< "${case_root}/b.txt")" == "${expected}-b" ]]
}

fault_case prepare-directory old 222222222222222222222222
fault_case journal old 333333333333333333333333
fault_case backup old 444444444444444444444444
fault_case promotion old 555555555555555555555555
fault_case commit new 666666666666666666666666
fault_case cleanup new 777777777777777777777777

# Completed journals are cleanup-only state. Recovery remains resumable if a
# crash already removed some or all of their journal records.
cleanup_root="${TEST_DIR}/partial-completed-cleanup"
committed="${cleanup_root}/.deploy-transactions/888888888888888888888888.committed"
rolled_back="${cleanup_root}/.deploy-transactions/999999999999999999999999.rolled-back"
mkdir -p "${committed}/backup/nested" "${rolled_back}"
printf 'stale\n' > "${committed}/backup/nested/a.txt"
printf 'version=3\nnested/a.txt\n' > "${committed}/manifest-v3.txt"
bash "${TRANSACTION}" recover "${cleanup_root}"
[[ ! -d "${cleanup_root}/.deploy-transactions" ]]

# Recovery reads the transaction's own manifest rather than a future release's.
recover_root="${TEST_DIR}/recover-own-manifest"
journal="${recover_root}/.deploy-transactions/888888888888888888888888.prepared"
mkdir -p "${recover_root}/nested" "${journal}/backup/nested"
printf 'new\n' > "${recover_root}/nested/a.txt"
printf 'old\n' > "${journal}/backup/nested/a.txt"
printf 'version=3\nnested/a.txt\n' > "${journal}/manifest-v3.txt"
printf 'nested/a.txt\tpresent\n' > "${journal}/state.tsv"
touch "${journal}/ready"
bash "${TRANSACTION}" recover "${recover_root}"
[[ "$(< "${recover_root}/nested/a.txt")" == "old" ]]

# Traversal and symlinked final ancestors fail closed.
unsafe_root="${TEST_DIR}/unsafe"
unsafe_token="999999999999999999999999"
mkdir -p "${unsafe_root}/.deploy-staging/${unsafe_token}/deploy"
printf 'version=3\n../escape\n' \
    > "${unsafe_root}/.deploy-staging/${unsafe_token}/deploy/manifest-v3.txt"
if promote "${TRANSACTION}" "${unsafe_root}" "${unsafe_token}" >/dev/null 2>&1; then
    echo "FAIL: manifest traversal was accepted" >&2
    exit 1
fi

symlink_root="${TEST_DIR}/symlink"
symlink_token="aaaaaaaaaaaaaaaaaaaaaaaa"
outside="${TEST_DIR}/outside"
mkdir -p "${symlink_root}" "${outside}"
prepare_generation "${symlink_root}" "${symlink_token}" "new"
ln -s "${outside}" "${symlink_root}/nested"
if promote "${TRANSACTION}" "${symlink_root}" "${symlink_token}" >/dev/null 2>&1; then
    echo "FAIL: symlinked destination ancestor was accepted" >&2
    exit 1
fi
[[ ! -e "${outside}/a.txt" ]]

# A symlinked staging root must never redirect validation, promotion, or
# cleanup outside the deployment directory.
staging_link_root="${TEST_DIR}/staging-link"
staging_link_outside="${TEST_DIR}/staging-link-outside"
staging_link_token="bbbbbbbbbbbbbbbbbbbbbbbb"
mkdir -p "${staging_link_root}" "${staging_link_outside}"
ln -s "${staging_link_outside}" "${staging_link_root}/.deploy-staging"
prepare_generation "${staging_link_root}" "${staging_link_token}" "new"
if promote "${TRANSACTION}" "${staging_link_root}" "${staging_link_token}" >/dev/null 2>&1; then
    echo "FAIL: symlinked staging root was accepted" >&2
    exit 1
fi
[[ "$(< "${staging_link_outside}/${staging_link_token}/b.txt")" == "new-b" ]]

grep -q "deploy-transaction.sh.*recover" "${PROJECT_ROOT}/scripts/deploy.sh"
grep -q "deploy/transaction.sh.*recover" "${PROJECT_ROOT}/scripts/deploy.sh"
grep -q "sha256sum" "${PROJECT_ROOT}/scripts/deploy.sh"
grep -q "REMOTE_STAGING_ROOT.*deploy-staging" "${PROJECT_ROOT}/scripts/deploy.sh"

echo "PASS: manifest-v3 nested promotion is durable and recoverable"
