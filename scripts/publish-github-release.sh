#!/bin/bash
# Build the release artifacts locally, push their commit and version tag, and
# create the corresponding GitHub Release. No hosted CI runner is involved.

set -euo pipefail

export LC_ALL=C
umask 022

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "${PROJECT_ROOT}"

if [[ $# -ne 0 ]]; then
    echo "Usage: $0" >&2
    exit 2
fi

command -v gh >/dev/null 2>&1 || {
    echo "ERROR: GitHub CLI (gh) is required to publish a release." >&2
    echo "Install it, then authenticate with: gh auth login" >&2
    exit 1
}
if ! gh auth status --hostname github.com >/dev/null 2>&1; then
    echo "ERROR: GitHub CLI is not authenticated with github.com." >&2
    echo "Run: gh auth login" >&2
    exit 1
fi

# A release tag comes from the committed workspace version, never from a
# one-off build override inherited from the caller's shell.
VERSION="$(env -u RPI_OMT_CLIENT_VERSION ./scripts/detect-version.sh "${PROJECT_ROOT}")"
if [[ ! "${VERSION}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([._-][0-9A-Za-z][0-9A-Za-z._-]*)?$ ]]; then
    echo "ERROR: '${VERSION}' is not a releasable vMAJOR.MINOR.PATCH version." >&2
    exit 1
fi

# A release build must describe only HEAD. Ignored compiler and publisher
# outputs are allowed; any other tracked or untracked input is not.
worktree_status="$(git status --porcelain --untracked-files=normal)"
if [[ -n "${worktree_status}" ]]; then
    echo "ERROR: Refusing to release from a dirty worktree." >&2
    echo "Commit or remove every non-ignored change first." >&2
    exit 1
fi

HEAD_COMMIT="$(git rev-parse --verify HEAD)"
if git show-ref --verify --quiet "refs/tags/${VERSION}"; then
    TAG_COMMIT="$(git rev-parse --verify "refs/tags/${VERSION}^{commit}")"
    if [[ "${TAG_COMMIT}" != "${HEAD_COMMIT}" ]]; then
        echo "ERROR: ${VERSION} already tags ${TAG_COMMIT}, not HEAD ${HEAD_COMMIT}." >&2
        echo "Bump workspace.package.version before creating another release." >&2
        exit 1
    fi
fi

BRANCH="$(git symbolic-ref --quiet --short HEAD || true)"
if [[ -z "${BRANCH}" ]]; then
    echo "ERROR: A GitHub Release must be published from a branch, not detached HEAD." >&2
    exit 1
fi
UPSTREAM="$(git for-each-ref --format='%(upstream:short)' "refs/heads/${BRANCH}")"
if [[ -z "${UPSTREAM}" || "${UPSTREAM}" != */* ]]; then
    echo "ERROR: Branch ${BRANCH} has no upstream. Push it with -u before releasing." >&2
    exit 1
fi
REMOTE="${UPSTREAM%%/*}"
REMOTE_BRANCH="${UPSTREAM#*/}"

# A completed release is immutable and does not need another expensive local
# build. The tag-to-HEAD check above still runs first so this cannot hide a
# reused workspace version.
if git show-ref --verify --quiet "refs/tags/${VERSION}" && \
   gh release view "${VERSION}" >/dev/null 2>&1; then
    echo "GitHub Release ${VERSION} already exists; existing assets were not changed."
    exit 0
fi

echo "=== Local release build: ${VERSION} from ${HEAD_COMMIT:0:12} ==="
# The deployers compile the appliance archive into themselves, so this order
# is part of the release contract even when make is invoked with parallelism.
for target in build-arm64 build-deployer build-windows-deployer; do
    echo "--- make ${target} ---"
    RPI_OMT_CLIENT_VERSION="${VERSION}" make "${target}"
done

LINUX_PUBLISH="${PROJECT_ROOT}/.build/deployer-publish"
WINDOWS_PUBLISH="${PROJECT_ROOT}/.build/deployer-publish-windows"
for required in \
    "${LINUX_PUBLISH}/bin/rpi-omt-deploy" \
    "${LINUX_PUBLISH}/bin/rpi-omt-deploy-tui" \
    "${LINUX_PUBLISH}/deployer-sbom.cdx.json" \
    "${WINDOWS_PUBLISH}/bin/rpi-omt-deploy.exe" \
    "${WINDOWS_PUBLISH}/bin/rpi-omt-deployer.exe" \
    "${WINDOWS_PUBLISH}/deployer-sbom.cdx.json"; do
    if [[ ! -f "${required}" || -L "${required}" || ! -s "${required}" ]]; then
        echo "ERROR: Release build did not produce ${required}." >&2
        exit 1
    fi
done

RELEASE_DIR="${PROJECT_ROOT}/.build/github-release/${VERSION}"
rm -rf -- "${RELEASE_DIR}"
mkdir -p -- "${RELEASE_DIR}"
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"
LINUX_ARCHIVE="${RELEASE_DIR}/rpi-omt-deployer-${VERSION}-linux-x86_64.tar.gz"
WINDOWS_ARCHIVE="${RELEASE_DIR}/rpi-omt-deployer-${VERSION}-windows-x86_64.tar.gz"
CHECKSUMS="${RELEASE_DIR}/SHA256SUMS"

# Fixed ownership, ordering, and timestamps make a retry byte-reproducible.
tar --sort=name --mtime="@${SOURCE_DATE_EPOCH}" --owner=0 --group=0 \
    --numeric-owner -czf "${LINUX_ARCHIVE}" -C "${LINUX_PUBLISH}" .
tar --sort=name --mtime="@${SOURCE_DATE_EPOCH}" --owner=0 --group=0 \
    --numeric-owner -czf "${WINDOWS_ARCHIVE}" -C "${WINDOWS_PUBLISH}" .
(
    cd "${RELEASE_DIR}"
    sha256sum "$(basename -- "${LINUX_ARCHIVE}")" \
        "$(basename -- "${WINDOWS_ARCHIVE}")" > "$(basename -- "${CHECKSUMS}")"
)

if ! git show-ref --verify --quiet "refs/tags/${VERSION}"; then
    git tag --annotate "${VERSION}" --message "Release ${VERSION}"
fi

# The API cannot attach a release to a commit GitHub has not received. Pushing
# the branch and tag atomically prevents a tag from landing without its commit.
git push --atomic "${REMOTE}" \
    "HEAD:refs/heads/${REMOTE_BRANCH}" "refs/tags/${VERSION}"

if gh release view "${VERSION}" >/dev/null 2>&1; then
    echo "GitHub Release ${VERSION} already exists; existing assets were not changed."
    exit 0
fi

release_flags=(--verify-tag --generate-notes --title "${VERSION}")
if [[ "${VERSION}" =~ ^v0\. ]]; then
    release_flags+=(--prerelease)
fi
gh release create "${VERSION}" \
    "${LINUX_ARCHIVE}" "${WINDOWS_ARCHIVE}" "${CHECKSUMS}" \
    "${release_flags[@]}"

echo "=== GitHub Release published: ${VERSION} ==="
