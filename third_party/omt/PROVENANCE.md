# Open Media Transport provenance

The Raspberry Pi OMT Client vendors the following upstream MIT-licensed
components. Build outputs, repository metadata, and the upstream `omtplayer`
web server are intentionally excluded.

| Component | Upstream | Commit |
|---|---|---|
| `omtplayer` playback core | https://github.com/openmediatransport/omtplayer | `c47397aa74600f998d9fa8533a26b0dbbf9e26e9` |
| `libomtnet` | https://github.com/openmediatransport/libomtnet | `bda284e86ea56166b0caa45f30136def8c893e5e` |
| `libvmx` | https://github.com/openmediatransport/libvmx | `f73569e767b9d9177519bf5765c9434dfe8af51f` |

Local integration changes are limited to target-framework/build integration,
DRM connector selection, lifecycle/status reporting, and the hardened command
line interface in `receiver/`.
