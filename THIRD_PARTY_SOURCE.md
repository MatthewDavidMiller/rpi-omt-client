# Third-party source availability

The release process produces an SBOM from the final Rust deployers and
Alpine runtime image. For each GPL/LGPL component in the runtime image, the
release bundle must include the exact Alpine source package, its `APKBUILD`,
patches, and build metadata under `third-party-source/alpine/`.

The complete project-owned transport, playback, and decoder source is in the
Cargo workspace under `crates/`. Exact upstream and historical derivation
revisions are recorded in `third_party/omt/PROVENANCE.md`; only upstream legal
and provenance records are retained under `third_party/omt/`.

The deployer source inputs are pinned by registry version and checksum in
`Cargo.lock`. Release source bundles must include the corresponding Cargo
registry archives or an equivalent approved mirror.

Do not publish or distribute a release when `scripts/check-legal-notices.py`
reports a missing license, notice, source record, or dependency.
