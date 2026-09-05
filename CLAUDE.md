# CLAUDE.md

Guidance for Claude Code (claude.ai/code) lives in one file shared by every
coding agent, so the two cannot drift. Read and follow it:

@AGENTS.md

`.claude/settings.json` denies reads of `vars.yml` and `env/**`. That is
intentional; do not work around it.
