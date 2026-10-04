#!/bin/bash
# Structural checks for the C build, its supply-chain policy, and the release gates.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PASS=0 FAIL=0
pass(){ printf 'PASS: %s\n' "$1"; PASS=$((PASS+1)); }
fail(){ printf 'FAIL: %s\n' "$1" >&2; FAIL=$((FAIL+1)); }
literal(){ grep -Fq -- "$2" "$1" && pass "$3" || fail "$3"; }
executable(){ [[ -x "$1" ]] && pass "$2" || fail "$2"; }
absent(){ [[ ! -e "$1" ]] && pass "$2" || fail "$2"; }

echo "=== Supply Chain Guardrail Tests ==="
absent "${PROJECT_ROOT}/CMakeLists.txt" "CMake entry point is absent"
absent "${PROJECT_ROOT}/Cargo.toml" "The Rust workspace is gone"
absent "${PROJECT_ROOT}/crates" "No Rust crates remain"
"${PROJECT_ROOT}/scripts/check-no-c-sources.sh" >/dev/null && pass "C sources stay in their trees" || fail "C sources stay in their trees"
literal "${PROJECT_ROOT}/VERSION" '.' "The canonical version file exists"
literal "${PROJECT_ROOT}/mk/flags.mk" '-std=c17' "The tree is C17"
literal "${PROJECT_ROOT}/mk/flags.mk" '-include src/common/banned.h' "Every C file is compiled with the banned-function list"
literal "${PROJECT_ROOT}/mk/flags.mk" '-Werror' "C warnings are errors"
literal "${PROJECT_ROOT}/mk/flags.mk" '-fsanitize=address,undefined' "Sanitizer builds exist"
literal "${PROJECT_ROOT}/mk/flags.mk" '-fanalyzer' "The GCC static analyzer runs"
literal "${PROJECT_ROOT}/src/common/banned.h" '#pragma GCC poison strcpy' "Unbounded string copies are poisoned"
literal "${PROJECT_ROOT}/src/protocol/omt.h" 'OMT_VIDEO_MAX_SIZE (10u * 1024u * 1024u)' "Video frames are bounded"
literal "${PROJECT_ROOT}/src/vmx/vmx.h" 'VMX_WORKER_STACK_SIZE (128u * 1024u)' "VMX worker stacks are bounded"
literal "${PROJECT_ROOT}/src/receiver_core/core.h" 'OMT_HEARTBEAT_MS 500u' "Status heartbeat remains 500 ms"
literal "${PROJECT_ROOT}/src/deploy/core/sys.h" 'DP_OUTPUT_LIMIT (4u * 1024u * 1024u)' "Deployer output is bounded"
literal "${PROJECT_ROOT}/src/deploy/core/deploy.h" 'DP_ACTION_REBOOT' "Management actions are a fixed set"
literal "${PROJECT_ROOT}/src/deploy/core/core.c" 'omt_pbkdf2_sha1(' "WPA PSKs are derived locally"
literal "${PROJECT_ROOT}/src/deploy/cli/main.c" 'omt_json_only_keys(' "Secrets input rejects unknown fields"
literal "${PROJECT_ROOT}/src/deploy/capsule/capsule.c" 'dp_license_data' "Deployer legal text is embedded"
literal "${PROJECT_ROOT}/src/deploy/ssh/kex.c" 'kex-strict-s-v00@openssh.com' "Strict key exchange is required"
literal "${PROJECT_ROOT}/src/deploy/ssh/kex.c" 'ssh_known_hosts_check(' "Deployer verifies OpenSSH known_hosts"
literal "${PROJECT_ROOT}/src/deploy/core/ops.c" 'wpa_cli -i \"$iface\"' "Wi-Fi updates use wpa_cli"
literal "${PROJECT_ROOT}/scripts/build-openssl.sh" 'OPENSSL_SHA256="' "The Windows OpenSSL is checksum pinned"
literal "${PROJECT_ROOT}/scripts/generate-runtime-sbom.py" 'RUNTIME_ROOTS = ["omt-receiver", "omt-web"]' "Runtime SBOM is scoped to both appliance binaries"
literal "${PROJECT_ROOT}/scripts/generate-deployer-sbom.py" 'DEPLOYER_ROOTS = ["rpi-omt-deploy", "rpi-omt-deploy-tui"]' "Deployer SBOM is scoped to the deployer"
literal "${PROJECT_ROOT}/deploy/Dockerfile" 'alpine:3.23.5@${ALPINE_DIGEST} AS c-builder' "Container builder is digest pinned"
literal "${PROJECT_ROOT}/deploy/Dockerfile" 'FROM scratch AS runtime-artifacts' "Runtime export omits its toolchain"
literal "${PROJECT_ROOT}/deploy/Dockerfile" '--apk-installed /tmp/runtime-installed' "Runtime SBOM inventories the installed packages"
literal "${PROJECT_ROOT}/deploy/compose.yml" 'mem_limit: "${OMT_CONTAINER_MEMORY_LIMIT:-512m}"' "Runtime memory is bounded"
literal "${PROJECT_ROOT}/deploy/compose.yml" 'pids_limit: 64' "Runtime process count is bounded"
executable "${PROJECT_ROOT}/tools/test-receiver.sh" "Receiver gate is executable"
executable "${PROJECT_ROOT}/tools/test-web.sh" "Web gate is executable"
executable "${PROJECT_ROOT}/scripts/check-c.sh" "C static gate is executable"
executable "${PROJECT_ROOT}/scripts/check-deployer.sh" "Deployer gate is executable"
executable "${PROJECT_ROOT}/scripts/build-windows-deployer.sh" "Windows deployer gate is executable"
executable "${PROJECT_ROOT}/scripts/build-openssl.sh" "Pinned OpenSSL build is executable"
executable "${PROJECT_ROOT}/tests/integration/test_ssh_client.sh" "SSH interop gate is executable"
executable "${PROJECT_ROOT}/scripts/publish-github-release.sh" "Local GitHub Release publisher is executable"
executable "${PROJECT_ROOT}/scripts/check-no-c-sources.sh" "C source placement gate is executable"
literal "${PROJECT_ROOT}/Makefile" 'test-receiver:' "Make exposes test-receiver"
literal "${PROJECT_ROOT}/Makefile" 'test-deployer:' "Make exposes test-deployer"
literal "${PROJECT_ROOT}/scripts/security-scan.sh" '--skip-files vars.yml' "Security scan excludes vars.yml"

executable "${PROJECT_ROOT}/scripts/check-supply-chain.sh" "Supply-chain gate is executable"
literal "${PROJECT_ROOT}/scripts/check-supply-chain.sh" 'ALLOWED_HEADER=' "System headers are allowlisted by a gate"
"${PROJECT_ROOT}/scripts/check-supply-chain.sh" >/dev/null && pass "Supply-chain gate passes" || fail "Supply-chain gate passes"

skip_escapes="$(grep -rInE 'SKIP_RETURN_CODE|pytest\.(skip|mark\.skip|mark\.xfail)|SKIP\$\{NC\}|"SKIP:' "${PROJECT_ROOT}/tests" "${PROJECT_ROOT}/scripts" "${PROJECT_ROOT}/tools" --include='*.sh' --include='*.py' --exclude-dir=.venv --exclude="$(basename -- "${BASH_SOURCE[0]}")" || true)"
[[ -z "${skip_escapes}" ]] && pass "No gate silently skips work" || { printf '%s\n' "${skip_escapes}" >&2; fail "No gate silently skips work"; }
literal "${PROJECT_ROOT}/tests/conftest.py" 'session.exitstatus = 1' "Python suite fails on excused cases"

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ ${FAIL} -eq 0 ]]
