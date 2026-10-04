# Third-party source availability

The release process produces an SBOM from the final deployers and the Alpine
runtime image. For each GPL/LGPL component in the runtime image, the
release bundle must include the exact Alpine source package, its `APKBUILD`,
patches, and build metadata under `third-party-source/alpine/`.

The complete project-owned transport, playback, decoder, Web, and deployer
source is the C tree under `src/`. Exact upstream and historical derivation
revisions are recorded in `third_party/omt/PROVENANCE.md`; only upstream legal
and provenance records are retained under `third_party/omt/`.

The only third-party source the deployers are built from is OpenSSL: Alpine's
package on Linux, and on Windows the release tarball pinned by version and
SHA-256 in `scripts/build-openssl.sh`. Release source bundles must include that
tarball.

Do not publish or distribute a release when `scripts/check-legal-notices.py`
reports a missing license, notice, source record, or dependency.
