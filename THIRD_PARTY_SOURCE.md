# Third-party source availability

The release process produces an SBOM from the final native deployer and
Alpine runtime image. For each GPL/LGPL component in the runtime image, the
release bundle must include the exact Alpine source package, its `APKBUILD`,
patches, and build metadata under `third-party-source/alpine/`.

The complete project-owned native transport and playback source is present in
`src/native/omt/` and `src/native/receiver/`. The vendored VMX decoder is in
`third_party/omt/libvmx/`; exact upstream and historical derivation revisions
are recorded in `third_party/omt/PROVENANCE.md`. No C# snapshot is part of the
build or release source tree.

The deployer source inputs are pinned by URL and SHA-256 in
`cmake/NativeDependencies.cmake`. Release source bundles must include the
corresponding SDL3, Dear ImGui, and libssh2 archives.

Do not publish or distribute a release when `scripts/check-legal-notices.py`
reports a missing license, notice, source record, or dependency.
