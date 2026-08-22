#!/bin/bash
# Exercise the local GitHub Release publisher without network calls or builds.

set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

REPO="${TEST_ROOT}/repo"
REMOTE="${TEST_ROOT}/remote.git"
mkdir -p "${REPO}/scripts" "${TEST_ROOT}/bin"
cp "${PROJECT_ROOT}/scripts/publish-github-release.sh" "${REPO}/scripts/"
cp "${PROJECT_ROOT}/scripts/detect-version.sh" "${REPO}/scripts/"
chmod +x "${REPO}/scripts/"*.sh

cat > "${REPO}/Cargo.toml" <<'EOF'
[workspace]

[workspace.package]
version = "0.2.3"
EOF
cat > "${REPO}/.gitignore" <<'EOF'
/.build/
/omt-client-arm64.tar.gz
EOF

git init --quiet --bare "${REMOTE}"
git -C "${REPO}" init --quiet
git -C "${REPO}" config user.name "Release Test"
git -C "${REPO}" config user.email "release@example.invalid"
git -C "${REPO}" add Cargo.toml .gitignore scripts
git -C "${REPO}" commit --quiet --message "release fixture"
git -C "${REPO}" remote add origin "${REMOTE}"
git -C "${REPO}" push --quiet --set-upstream origin HEAD

mkdir -p \
    "${REPO}/.build/deployer-publish/bin" \
    "${REPO}/.build/deployer-publish-windows/bin"
for artifact in \
    .build/deployer-publish/bin/rpi-omt-deploy \
    .build/deployer-publish/bin/rpi-omt-deploy-tui \
    .build/deployer-publish/deployer-sbom.cdx.json \
    .build/deployer-publish-windows/bin/rpi-omt-deploy.exe \
    .build/deployer-publish-windows/bin/rpi-omt-deployer.exe \
    .build/deployer-publish-windows/deployer-sbom.cdx.json; do
    printf 'fixture artifact: %s\n' "${artifact}" > "${REPO}/${artifact}"
done

cat > "${TEST_ROOT}/bin/make" <<'EOF'
#!/bin/bash
printf '%s\n' "$1" >> "${MAKE_LOG}"
EOF
cat > "${TEST_ROOT}/bin/gh" <<'EOF'
#!/bin/bash
printf '%s\n' "$*" >> "${GH_LOG}"
case "${1:-} ${2:-}" in
    "auth status") exit 0 ;;
    "release view") [[ -e "${GH_RELEASE_STATE}" ]] ;;
    "release create") touch "${GH_RELEASE_STATE}" ;;
    *) exit 1 ;;
esac
EOF
chmod +x "${TEST_ROOT}/bin/make" "${TEST_ROOT}/bin/gh"

MAKE_LOG="${TEST_ROOT}/make.log" \
GH_LOG="${TEST_ROOT}/gh.log" \
GH_RELEASE_STATE="${TEST_ROOT}/release-created" \
PATH="${TEST_ROOT}/bin:${PATH}" \
    "${REPO}/scripts/publish-github-release.sh" >/dev/null

make_order="$(tr '\n' ' ' < "${TEST_ROOT}/make.log")"
if [[ "${make_order}" != \
      "build-arm64 build-deployer build-windows-deployer " ]]; then
    echo "FAIL: release builds did not run in dependency order" >&2
    exit 1
fi

HEAD_COMMIT="$(git -C "${REPO}" rev-parse HEAD)"
REMOTE_TAG_COMMIT="$(git --git-dir="${REMOTE}" rev-parse 'refs/tags/v0.2.3^{commit}')"
[[ "${REMOTE_TAG_COMMIT}" == "${HEAD_COMMIT}" ]] || {
    echo "FAIL: version tag was not pushed at the release commit" >&2
    exit 1
}

RELEASE_DIR="${REPO}/.build/github-release/v0.2.3"
LINUX_ARCHIVE="${RELEASE_DIR}/rpi-omt-deployer-v0.2.3-linux-x86_64.tar.gz"
WINDOWS_ARCHIVE="${RELEASE_DIR}/rpi-omt-deployer-v0.2.3-windows-x86_64.tar.gz"
tar -tzf "${LINUX_ARCHIVE}" | grep -Fq './bin/rpi-omt-deploy-tui'
tar -tzf "${WINDOWS_ARCHIVE}" | grep -Fq './bin/rpi-omt-deployer.exe'
(cd "${RELEASE_DIR}" && sha256sum --check SHA256SUMS >/dev/null)
grep -Fq 'release create v0.2.3' "${TEST_ROOT}/gh.log"
grep -Fq -- '--verify-tag --generate-notes --title v0.2.3 --prerelease' \
    "${TEST_ROOT}/gh.log"

echo "PASS: local builds produce a tagged prerelease with verified packages"

# A retry recognizes the release and must not issue another create operation or
# mutate its published assets.
MAKE_LOG="${TEST_ROOT}/make.log" \
GH_LOG="${TEST_ROOT}/gh.log" \
GH_RELEASE_STATE="${TEST_ROOT}/release-created" \
PATH="${TEST_ROOT}/bin:${PATH}" \
    "${REPO}/scripts/publish-github-release.sh" >/dev/null
create_count="$(grep -Fc 'release create v0.2.3' "${TEST_ROOT}/gh.log")"
[[ "${create_count}" == 1 ]] || {
    echo "FAIL: retry replaced an existing GitHub Release" >&2
    exit 1
}
retry_build_count="$(wc -l < "${TEST_ROOT}/make.log")"
[[ "${retry_build_count}" == 3 ]] || {
    echo "FAIL: retry rebuilt an already completed GitHub Release" >&2
    exit 1
}

echo "PASS: an existing GitHub Release remains immutable"

# Reusing the workspace version on a later commit would move a released tag.
# Refuse it before any expensive build begins.
printf 'later commit\n' > "${REPO}/tracked"
git -C "${REPO}" add tracked
git -C "${REPO}" commit --quiet --message "later"
build_count="$(wc -l < "${TEST_ROOT}/make.log")"
if MAKE_LOG="${TEST_ROOT}/make.log" \
    GH_LOG="${TEST_ROOT}/gh.log" \
    GH_RELEASE_STATE="${TEST_ROOT}/release-created" \
    PATH="${TEST_ROOT}/bin:${PATH}" \
        "${REPO}/scripts/publish-github-release.sh" >/dev/null 2>&1; then
    echo "FAIL: release publisher reused a version tag on another commit" >&2
    exit 1
fi
final_build_count="$(wc -l < "${TEST_ROOT}/make.log")"
[[ "${final_build_count}" == "${build_count}" ]] || {
    echo "FAIL: tag collision was detected only after rebuilding" >&2
    exit 1
}

echo "PASS: a released version cannot be moved to another commit"
