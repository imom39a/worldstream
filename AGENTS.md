# Repository guidance

Read `CONTEXT.md` and the relevant decisions in `docs/adr/` before changing Room
semantics. `docs/adr/0044-preserve-kernel-and-examples.md` defines the current
kernel-and-examples scope and supersedes earlier product/release plans.

Keep domain rules in Activity Packs and provider execution in external Runners.
Preserve exact revision identities, retained executors, and conformance fixtures;
they are inputs to Replay, not disposable generated output.

Run `scripts/verify-local.sh` for Rust changes. See `docs/gates.md` for optional
SDK and example checks. Update moved paths and verify documentation links when
reorganizing files.

The public site is plain HTML/CSS in `site/`, published by GitHub Pages. Keep it
independent of application backends and package installation.

Use ASD-STE100 principles for documentation and site text. Use short sentences,
active voice, consistent technical names, and direct instructions. Do not add
marketing slogans. Keep protocol identifiers and code examples exact.
