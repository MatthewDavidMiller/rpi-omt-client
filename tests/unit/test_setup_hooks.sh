#!/bin/bash
# Verify local hook setup references the tracked hooks rather than stale copies.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "${TEST_DIR}"' EXIT

mkdir -p "${TEST_DIR}/scripts" "${TEST_DIR}/.githooks"
cp "${PROJECT_ROOT}/scripts/setup-hooks.sh" "${TEST_DIR}/scripts/setup-hooks.sh"
cp "${PROJECT_ROOT}/.githooks/pre-commit" "${TEST_DIR}/.githooks/pre-commit"
cp "${PROJECT_ROOT}/.githooks/post-commit" "${TEST_DIR}/.githooks/post-commit"
git -C "${TEST_DIR}" init -q

"${TEST_DIR}/scripts/setup-hooks.sh" >/dev/null

[[ "$(git -C "${TEST_DIR}" config --local core.hooksPath)" == ".githooks" ]]
[[ -x "${TEST_DIR}/.githooks/pre-commit" ]]
[[ -x "${TEST_DIR}/.githooks/post-commit" ]]
[[ ! -e "${TEST_DIR}/.git/hooks/pre-commit" ]]
[[ ! -e "${TEST_DIR}/.git/hooks/post-commit" ]]

echo "PASS: tracked hooks are configured through core.hooksPath"

# A setup that installs only one of the two hooks would leave a workstation
# testing every commit and publishing none of them, or the reverse.
rm "${TEST_DIR}/.githooks/post-commit"
if "${TEST_DIR}/scripts/setup-hooks.sh" >/dev/null 2>&1; then
    echo "FAIL: setup accepted a missing post-commit hook" >&2
    exit 1
fi

echo "PASS: setup requires every tracked hook"

# The commit gate proves the builds; the publishing builds run after the commit
# so an artifact's baked-in version is the one its commit carries.
if grep -Fq -- '--publish' "${PROJECT_ROOT}/.githooks/pre-commit"; then
    echo "FAIL: pre-commit publishes an artifact" >&2
    exit 1
fi
for target in build-arm64 build-deployer build-windows-deployer; do
    grep -Fq "${target}" "${PROJECT_ROOT}/.githooks/post-commit" || {
        echo "FAIL: post-commit does not run ${target}" >&2
        exit 1
    }
done

echo "PASS: publishing builds run after the commit, not before"

# Drive the real hook against a throwaway repository with a recording `make`,
# so the ordering and the sequencer guard are checked without spending an
# emulated image build on a unit suite.
HOOK_DIR="$(mktemp -d)"
trap 'rm -rf "${TEST_DIR}" "${HOOK_DIR}"' EXIT

mkdir -p "${HOOK_DIR}/repo/.githooks" "${HOOK_DIR}/repo/scripts" "${HOOK_DIR}/bin"
cp "${PROJECT_ROOT}/.githooks/post-commit" "${HOOK_DIR}/repo/.githooks/post-commit"
cp "${PROJECT_ROOT}/scripts/detect-version.sh" "${HOOK_DIR}/repo/scripts/detect-version.sh"
chmod +x "${HOOK_DIR}/repo/.githooks/post-commit"
cat > "${HOOK_DIR}/bin/make" <<'EOF'
#!/bin/bash
printf '%s\n' "$1" >> "${MAKE_LOG}"
EOF
chmod +x "${HOOK_DIR}/bin/make"
git -C "${HOOK_DIR}/repo" init -q
git -C "${HOOK_DIR}/repo" config --local core.hooksPath .githooks
git -C "${HOOK_DIR}/repo" config --local user.email hook@example.invalid
git -C "${HOOK_DIR}/repo" config --local user.name "Hook Test"
printf 'v1.2.3\n' > "${HOOK_DIR}/repo/tracked"
git -C "${HOOK_DIR}/repo" add tracked

MAKE_LOG="${HOOK_DIR}/make-log" PATH="${HOOK_DIR}/bin:${PATH}" \
    git -C "${HOOK_DIR}/repo" commit -q -m "first" > "${HOOK_DIR}/commit-output" 2>&1

# The deployer executables compile the image into themselves, so the image has
# to be built before either of them.
if [[ "$(tr '\n' ' ' < "${HOOK_DIR}/make-log")" != \
      "build-arm64 build-deployer build-windows-deployer " ]]; then
    echo "FAIL: post-commit did not build the image before the deployers" >&2
    cat "${HOOK_DIR}/commit-output" >&2
    exit 1
fi

echo "PASS: post-commit publishes in dependency order after a commit"

# A replayed commit is an intermediate state; publishing on each one rebuilds
# the image repeatedly and leaves the packages describing the last replay.
rm -f "${HOOK_DIR}/make-log"
mkdir -p "${HOOK_DIR}/repo/.git/rebase-merge"
printf 'v1.2.4\n' > "${HOOK_DIR}/repo/tracked"
git -C "${HOOK_DIR}/repo" add tracked
MAKE_LOG="${HOOK_DIR}/make-log" PATH="${HOOK_DIR}/bin:${PATH}" \
    git -C "${HOOK_DIR}/repo" commit -q -m "replayed" > "${HOOK_DIR}/replay-output" 2>&1
rmdir "${HOOK_DIR}/repo/.git/rebase-merge"

if [[ -e "${HOOK_DIR}/make-log" ]]; then
    echo "FAIL: post-commit published during a rebase" >&2
    cat "${HOOK_DIR}/replay-output" >&2
    exit 1
fi

echo "PASS: post-commit skips commits replayed by a sequencer"
