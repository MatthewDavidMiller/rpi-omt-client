#!/bin/bash
# Verify test-local rejects options that could silently select a smaller suite.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RUNNER="${PROJECT_ROOT}/scripts/test-local.sh"

if "${RUNNER}" --ful >/dev/null 2>&1; then
    echo "FAIL: misspelled mode was accepted" >&2
    exit 1
fi
if "${RUNNER}" --quick extra >/dev/null 2>&1; then
    echo "FAIL: extra mode argument was accepted" >&2
    exit 1
fi
if ! "${RUNNER}" --help | grep -q '^Usage:'; then
    echo "FAIL: help output is unavailable" >&2
    exit 1
fi

echo "PASS: test-local accepts only documented modes"

case_dir=$(mktemp -d)
trap 'rm -rf "${case_dir}"' EXIT
fixture_root="${case_dir}/project"
outside_dir="${case_dir}/outside"
mkdir -p \
    "${fixture_root}/scripts" \
    "${fixture_root}/tests/integration" \
    "${fixture_root}/tests/unit" \
    "${fixture_root}/tests/.venv/bin" \
    "${fixture_root}/tools" \
    "${fixture_root}/test-bin" \
    "${outside_dir}"
cp "${RUNNER}" "${fixture_root}/scripts/test-local.sh"
cp "${PROJECT_ROOT}/scripts/docker-test-env.sh" \
    "${fixture_root}/scripts/docker-test-env.sh"

# Every shell suite must be wired into the runner. One that is not would pass
# review, pass its own invocation, and never run in any gate again.
unwired=()
for suite in "${PROJECT_ROOT}"/tests/unit/*.sh; do
    suite_path="tests/unit/$(basename -- "${suite}")"
    grep -Fq "${suite_path}" "${RUNNER}" || unwired+=("${suite_path}")
done
if ((${#unwired[@]} > 0)); then
    echo "FAIL: shell suites are not run by test-local.sh: ${unwired[*]}" >&2
    exit 1
fi
echo "PASS: every tests/unit shell suite is wired into the runner"

stub_paths=(
    scripts/lint.sh
    tools/test-receiver.sh
    tests/integration/test_container_smoke.sh
    tests/integration/test_omt_network.sh
)
for suite in "${PROJECT_ROOT}"/tests/unit/*.sh; do
    stub_paths+=("tests/unit/$(basename -- "${suite}")")
done

cat > "${fixture_root}/tests/integration/test_docker_build.sh" <<'EOF'
#!/bin/bash
printf 'ran\n' > "${DOCKER_BUILD_LOG}"
EOF
chmod +x "${fixture_root}/tests/integration/test_docker_build.sh"

cat > "${fixture_root}/scripts/build-windows-deployer.sh" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" >> "${WINDOWS_BUILD_LOG}"
EOF
chmod +x "${fixture_root}/scripts/build-windows-deployer.sh"
for stub_path in "${stub_paths[@]}"; do
    ln -s /bin/true "${fixture_root}/${stub_path}"
done

cat > "${fixture_root}/scripts/check-deployer.sh" <<'EOF'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "${CHECK_DEPLOYER_LOG}"
EOF
chmod +x "${fixture_root}/scripts/check-deployer.sh"

cat > "${fixture_root}/test-bin/docker" <<'EOF'
#!/bin/bash
exit 0
EOF
chmod +x "${fixture_root}/test-bin/docker"

cat > "${fixture_root}/test-bin/cargo" <<'EOF'
#!/bin/bash
exit 0
EOF
chmod +x "${fixture_root}/test-bin/cargo"

cat > "${fixture_root}/tests/.venv/bin/python" <<'EOF'
#!/bin/bash
set -euo pipefail
[[ "${PWD}" == "${EXPECTED_PROJECT_ROOT}" ]]
[[ "${1:-}" == "-m" ]]
[[ "${2:-}" == "pytest" ]]
[[ "${3:-}" == "tests/unit/test_cross_file_invariants.py" ]]
printf '%s\n' "${PWD}" > "${PROBE_RESULT}"
EOF
chmod +x "${fixture_root}/tests/.venv/bin/python"

probe_result="${case_dir}/pytest-cwd"
if (
    cd "${outside_dir}"
    PATH="${fixture_root}/test-bin:${PATH}" \
        EXPECTED_PROJECT_ROOT="${fixture_root}" \
        PROBE_RESULT="${probe_result}" \
        CHECK_DEPLOYER_LOG="${case_dir}/quick-deployer" \
        WINDOWS_BUILD_LOG="${case_dir}/quick-windows" \
        "${fixture_root}/scripts/test-local.sh" --quick \
        > "${case_dir}/runner-output" 2>&1
) && grep -Fxq "${fixture_root}" "${probe_result}"; then
    echo "PASS: test-local resolves relative test paths from its project root"
else
    echo "FAIL: test-local depends on the caller's working directory" >&2
    cat "${case_dir}/runner-output" >&2
    exit 1
fi

if [[ "$(wc -l < "${case_dir}/quick-deployer")" -eq 1 ]] &&
   [[ -z "$(< "${case_dir}/quick-deployer")" ]]; then
    echo "PASS: quick mode runs the deployer gate without publishing"
else
    echo "FAIL: quick mode deployer contract changed" >&2
    exit 1
fi

run_container_mode() {
    local mode="$1"
    local log_file="${case_dir}/${mode}-deployer"
    local -a mode_args=()
    [[ "${mode}" == "full" ]] && mode_args=(--full)
    (
        cd "${outside_dir}"
        PATH="${fixture_root}/test-bin:${PATH}" \
            CONTAINER_ENGINE=docker \
            DOCKER_BUILD_LOG="${case_dir}/${mode}-docker-build" \
            WINDOWS_BUILD_LOG="${case_dir}/${mode}-windows" \
            EXPECTED_PROJECT_ROOT="${fixture_root}" \
            PROBE_RESULT="${probe_result}" \
            CHECK_DEPLOYER_LOG="${log_file}" \
            "${fixture_root}/scripts/test-local.sh" "${mode_args[@]}" \
            > "${case_dir}/${mode}-output" 2>&1
    )
    # Every mode that reaches a container engine builds the image and both
    # deployer executables. A mode that quietly drops one of them would still
    # be green here without these three logs. Neither deployer build publishes
    # a package: `make build-deployer`, `make build-windows-deployer`, and the
    # post-commit hook that runs them own publishing, so an artifact's version
    # is the one its commit carries. The deployer gate is invoked exactly
    # once: the second invocation this used to require was the retired
    # `--integration-only` mode, which re-ran the tests the first one had
    # already run and reported it as an Alpine userland check.
    if [[ "$(wc -l < "${log_file}")" -eq 1 ]] &&
       [[ -z "$(< "${log_file}")" ]] &&
       grep -Fxq ran "${case_dir}/${mode}-docker-build" &&
       grep -Fxq -- '--no-publish' "${case_dir}/${mode}-windows"; then
        echo "PASS: ${mode} mode builds the image and both deployers without publishing"
    else
        echo "FAIL: ${mode} mode deployer contract changed" >&2
        cat "${case_dir}/${mode}-output" >&2
        exit 1
    fi
}

run_container_mode default
run_container_mode full

# Quick mode is the one gate allowed to omit container and cross builds. It
# still may not omit a Python or shell suite.
if [[ -e "${case_dir}/quick-windows" ]]; then
    echo "FAIL: quick mode ran the Windows cross build" >&2
    exit 1
fi
echo "PASS: quick mode omits only the container and cross builds"
