# Third-party source availability

The release process produces an SBOM from the final Windows deployer and
Alpine runtime image. For each GPL/LGPL component in the runtime image, the
release bundle must include the exact Alpine source package, its `APKBUILD`,
patches, and build metadata under `third-party-source/alpine/`.

The complete OMT source used by the receiver is present under
`third_party/omt/`; exact upstream revisions are recorded in
`third_party/omt/PROVENANCE.md`.

Do not publish or distribute a release when `scripts/check-legal-notices.py`
reports a missing license, notice, source record, or dependency.
