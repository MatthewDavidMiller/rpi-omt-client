# AGENTS.md

Raspberry Pi OMT Client receives Open Media Transport video and audio and
presents it directly on HDMI: a Rust 2024 workspace, a shell deployment
capsule, and a container image, targeting Alpine 3.24 aarch64 on a Raspberry
Pi 5 or Pi 4 Model B.

Start with **[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)** — workspace layout,
commands, the invariants the gates enforce, and which gate to run for what you
touched. It is written for contributors and agents alike; this file adds only
what is specific to working here as an agent.

- Prefer minimal, targeted changes. Coverage is dense and most contracts are
  asserted somewhere; a broad refactor usually breaks one that exists for a
  reason.
- Do not assume Raspberry Pi hardware is attached. Development is on amd64;
  Pi-only claims need the hardware checklist in
  [docs/TESTING.md](docs/TESTING.md).
- Gates are slow by design — an emulated ARM64 image build takes tens of
  minutes. Run the narrowest one rather than the broadest, and allow it to
  finish.
- Treat `vars.yml` and the local `env` directory as sensitive operator
  configuration; do not read or modify them unless asked.
- Commit or push only when asked. `git commit` triggers the full pre-commit
  gate and then rebuilds all three published artifacts through
  `.githooks/post-commit`.
- When finishing, report what changed, which gates ran, and any unrun check or
  Pi-specific risk that remains.
