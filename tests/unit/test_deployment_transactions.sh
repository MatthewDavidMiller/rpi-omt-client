#!/bin/bash
# Verify CLI builds preserve good output and deploys promote complete sets.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

case_root=$(mktemp -d)
trap 'rm -rf "${case_root}"' EXIT

pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }

# Exercise the same-directory temporary output used by the CLI ARM64 build.
build_root="${case_root}/build-project"
mkdir -p "${build_root}/scripts" "${build_root}/fake-bin"
cp "${PROJECT_ROOT}/scripts/build-arm64.sh" "${build_root}/scripts/"
printf '#!/bin/bash\nexit 0\n' > "${build_root}/scripts/check-arm64-emulation.sh"
chmod +x "${build_root}/scripts/check-arm64-emulation.sh"
cat > "${build_root}/fake-bin/docker" <<'EOF'
#!/bin/bash
set -euo pipefail

output_path=""
iid_path=""
while (($#)); do
    case "$1" in
        --output)
            output_path="${2#type=docker,dest=}"
            shift 2
            ;;
        --iidfile)
            iid_path="$2"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done
[[ -n "${output_path}" && -n "${iid_path}" ]]
case "${FAKE_BUILD_MODE:-success}" in
    fail)
        exit 42
        ;;
    invalid)
        printf 'not-a-tar' > "${output_path}"
        printf 'sha256:invalid\n' > "${iid_path}"
        ;;
    success)
        payload_dir=$(mktemp -d)
        trap 'rm -rf "${payload_dir}"' EXIT
        printf 'new-layer\n' > "${payload_dir}/layer"
        tar -cf "${output_path}" -C "${payload_dir}" layer
        printf 'sha256:0123456789abcdef\n' > "${iid_path}"
        ;;
    *)
        exit 2
        ;;
esac
EOF
chmod +x "${build_root}/fake-bin/docker"
printf 'known-good-tarball' > "${build_root}/omt-client-arm64.tar.gz"

if PATH="${build_root}/fake-bin:${PATH}" FAKE_BUILD_MODE=fail \
    "${build_root}/scripts/build-arm64.sh" >/dev/null 2>&1; then
    fail "failed ARM64 build unexpectedly succeeded"
fi
if [[ "$(< "${build_root}/omt-client-arm64.tar.gz")" == "known-good-tarball" ]] && \
   ! find "${build_root}" -maxdepth 1 -name '.omt-client-arm64.tar.gz.build.*' \
       -print -quit | grep -q .; then
    pass "failed ARM64 build preserves the existing artifact and cleans staging"
else
    fail "failed ARM64 build damaged the existing artifact or left staging"
fi

if PATH="${build_root}/fake-bin:${PATH}" FAKE_BUILD_MODE=invalid \
    "${build_root}/scripts/build-arm64.sh" >/dev/null 2>&1; then
    fail "invalid ARM64 build output unexpectedly succeeded"
fi
if [[ "$(< "${build_root}/omt-client-arm64.tar.gz")" == "known-good-tarball" ]]; then
    pass "invalid ARM64 archive is rejected before publication"
else
    fail "invalid ARM64 archive replaced the existing artifact"
fi

PATH="${build_root}/fake-bin:${PATH}" FAKE_BUILD_MODE=success \
    "${build_root}/scripts/build-arm64.sh" >/dev/null
if tar -tf "${build_root}/omt-client-arm64.tar.gz" | grep -Fxq layer && \
   grep -Fxq 'sha256:0123456789abcdef' "${build_root}/.build/arm64.iid"; then
    pass "verified ARM64 output is atomically published with its image identity"
else
    fail "successful ARM64 output was not published"
fi

# Run deploy.sh against local SSH/SCP doubles. The fake SSH executes the exact
# journal script received on stdin, translating only the remote root into /tmp.
deploy_root="${case_root}/deploy-project"
fake_bin="${deploy_root}/fake-bin"
mkdir -p "${deploy_root}/scripts" "${fake_bin}"
cp "${PROJECT_ROOT}/scripts/deploy.sh" "${deploy_root}/scripts/"
cp "${PROJECT_ROOT}/deploy-transaction.sh" "${deploy_root}/"
cp "${PROJECT_ROOT}/deploy-artifacts.txt" "${deploy_root}/"
mapfile -t artifact_names < "${deploy_root}/deploy-artifacts.txt"
for name in "${artifact_names[@]}"; do
    printf 'new-%s\n' "${name}" > "${deploy_root}/${name}"
done

cat > "${fake_bin}/scp" <<'EOF'
#!/bin/bash
set -euo pipefail

source_path="$1"
destination="$2"
remote_path="${destination#*:}"
case "${remote_path}" in
    /opt/omt-client/*)
        local_path="${FAKE_REMOTE_ROOT}${remote_path#/opt/omt-client}"
        ;;
    *)
        exit 2
        ;;
esac
cp -- "${source_path}" "${local_path}"
if [[ -n "${FAKE_CORRUPT_NAME:-}" && "${remote_path}" == *"${FAKE_CORRUPT_NAME}"* ]]; then
    printf 'corrupt\n' >> "${local_path}"
fi
EOF
chmod +x "${fake_bin}/scp"

cat > "${fake_bin}/ssh" <<'EOF'
#!/bin/bash
set -euo pipefail

if [[ "${1:-}" == "-t" ]]; then
    shift
fi
host="$1"
shift
[[ "${host}" == "pi@test-pi" ]]

map_path() {
    case "$1" in
        /opt/omt-client)
            printf '%s\n' "${FAKE_REMOTE_ROOT}"
            ;;
        /opt/omt-client/*)
            printf '%s%s\n' "${FAKE_REMOTE_ROOT}" "${1#/opt/omt-client}"
            ;;
        *)
            return 1
            ;;
    esac
}

if [[ $# -eq 2 && "$1" == "uname" && "$2" == "-m" ]]; then
    printf '%s\n' "${FAKE_ARCH:-aarch64}"
    exit 0
fi

if [[ $# -eq 1 && "$1" == sudo\ install\ -d* ]]; then
    mkdir -p "${FAKE_REMOTE_ROOT}"
    exit 0
fi
if [[ $# -eq 1 && "$1" == chmod\ +x* ]]; then
    printf 'installed\n' > "${FAKE_REMOTE_ROOT}/installer-invoked"
    exit 0
fi
if [[ "${1:-}" == "sha256sum" ]]; then
    shift
    [[ "${1:-}" == "--" ]] && shift
    sha256sum -- "$(map_path "$1")"
    exit 0
fi
if [[ "${1:-}" == "rm" ]]; then
    shift
    [[ "${1:-}" == "-f" ]] && shift
    [[ "${1:-}" == "--" ]] && shift
    for remote_path in "$@"; do
        rm -f -- "$(map_path "${remote_path}")"
    done
    exit 0
fi
if [[ "${1:-}" == "bash" && "${2:-}" == /opt/omt-client/.deploy-transaction.sh.upload-* ]]; then
    helper="$(map_path "$2")"
    command_name="$3"
    remote_dir="$(map_path "$4")"
    token="$5"
    manifest="$(map_path "$6")"
    bash "${helper}" "${command_name}" "${remote_dir}" "${token}" "${manifest}"
    exit $?
fi
printf 'unsupported fake SSH command: %s\n' "$*" >&2
exit 2
EOF
chmod +x "${fake_bin}/ssh"

remote_success="${case_root}/remote-success"
mkdir -p "${remote_success}/.deploy-transactions/interrupted.prepared"
mkdir -p "${remote_success}/.deploy-transactions/unready.prepared"
: > "${remote_success}/.deploy-transactions/unready.prepared/install.sh.present"
for name in "${artifact_names[@]}"; do
    printf 'old-%s\n' "${name}" > "${remote_success}/${name}"
    : > "${remote_success}/.deploy-transactions/interrupted.prepared/${name}.present"
done
mv "${remote_success}/omt-client-arm64.tar.gz" \
    "${remote_success}/.deploy-transactions/interrupted.prepared/omt-client-arm64.tar.gz.old"
printf 'partial-new\n' > "${remote_success}/omt-client-arm64.tar.gz"
: > "${remote_success}/.deploy-transactions/interrupted.prepared/ready"

PATH="${fake_bin}:${PATH}" FAKE_REMOTE_ROOT="${remote_success}" \
    "${deploy_root}/scripts/deploy.sh" pi@test-pi >/dev/null
for name in "${artifact_names[@]}"; do
    if ! cmp -s "${deploy_root}/${name}" "${remote_success}/${name}"; then
        fail "CLI deployment produced a mixed or incomplete artifact set"
    fi
done
if [[ -f "${remote_success}/installer-invoked" ]] && \
   { [[ ! -d "${remote_success}/.deploy-transactions" ]] || \
     ! find "${remote_success}/.deploy-transactions" -mindepth 1 -print -quit | \
         grep -q .; }; then
    pass "CLI deployment recovers, verifies, and promotes one complete set"
else
    fail "CLI deployment left a journal behind or skipped the installer"
fi

remote_mismatch="${case_root}/remote-mismatch"
mkdir -p "${remote_mismatch}"
for name in "${artifact_names[@]}"; do
    printf 'old-%s\n' "${name}" > "${remote_mismatch}/${name}"
done
if PATH="${fake_bin}:${PATH}" FAKE_REMOTE_ROOT="${remote_mismatch}" \
    FAKE_CORRUPT_NAME=docker-compose.yml \
    "${deploy_root}/scripts/deploy.sh" pi@test-pi >/dev/null 2>&1; then
    fail "CLI deployment accepted a checksum mismatch"
fi
for name in "${artifact_names[@]}"; do
    if [[ "$(< "${remote_mismatch}/${name}")" != "old-${name}" ]]; then
        fail "checksum mismatch changed an existing remote artifact"
    fi
done
if [[ ! -e "${remote_mismatch}/installer-invoked" ]] && \
   ! find "${remote_mismatch}" -maxdepth 1 -name '.*.upload-*' -print -quit | \
       grep -q .; then
    pass "checksum mismatch preserves the complete prior set and cleans staging"
else
    fail "checksum mismatch invoked install or left staged files"
fi

remote_wrong_arch="${case_root}/remote-wrong-arch"
if PATH="${fake_bin}:${PATH}" FAKE_REMOTE_ROOT="${remote_wrong_arch}" \
    FAKE_ARCH=x86_64 "${deploy_root}/scripts/deploy.sh" pi@test-pi \
        >/dev/null 2>&1; then
    fail "CLI deployment accepted a non-aarch64 target"
fi
if [[ ! -e "${remote_wrong_arch}" ]]; then
    pass "CLI architecture preflight rejects targets before remote mutation"
else
    fail "wrong-architecture preflight created or uploaded remote state"
fi

for phase in prepare-directory prepare-markers ready-fsync backup promote committed; do
    fault_root="${case_root}/fault-${phase}"
    mkdir -p "${fault_root}"
    fault_token=0123456789abcdef01234567
    staged_helper="${fault_root}/.deploy-transaction.sh.upload-${fault_token}"
    staged_manifest="${fault_root}/.deploy-artifacts.txt.upload-${fault_token}"
    sed "0,/# phase:${phase}/s//# phase:${phase}\nfalse # injected fault/" \
        "${PROJECT_ROOT}/deploy-transaction.sh" > "${staged_helper}"
    cp "${PROJECT_ROOT}/deploy-artifacts.txt" "${staged_manifest}"
    for name in "${artifact_names[@]}"; do
        printf 'old-%s\n' "${name}" > "${fault_root}/${name}"
        printf 'new-%s\n' "${name}" > \
            "${fault_root}/.${name}.upload-${fault_token}"
    done
    if bash "${staged_helper}" promote "${fault_root}" "${fault_token}" \
            "${staged_manifest}" >/dev/null 2>&1; then
        fail "fault injection unexpectedly succeeded at ${phase}"
    fi
    bash "${PROJECT_ROOT}/deploy-transaction.sh" recover "${fault_root}" \
        "${PROJECT_ROOT}/deploy-artifacts.txt"
    old_count=0
    new_count=0
    for name in "${artifact_names[@]}"; do
        grep -Fqx "old-${name}" "${fault_root}/${name}" && \
            old_count=$((old_count + 1))
        grep -Fqx "new-${name}" "${fault_root}/${name}" && \
            new_count=$((new_count + 1))
    done
    if [[ "${old_count}" -eq "${#artifact_names[@]}" || \
          "${new_count}" -eq "${#artifact_names[@]}" ]]; then
        pass "deployment fault recovery preserves a complete set at ${phase}"
    else
        fail "deployment fault recovery produced a mixed set at ${phase}"
    fi
done

printf 'All deployment transaction tests passed.\n'
