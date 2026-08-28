# Maintainer workflow

Make the smallest change at the owning seam, prove it locally, review against
both standards and the originating requirement, and keep exact-revision
compatibility honest.

## Before editing

1. Read `CONTEXT.md` and the affected ADRs.
2. Identify whether the change is canonical, operational, transport,
   presentation, or external policy.
3. Check the Linear issue and blocker frontier when work is ticketed.
4. Record the fixed comparison point for later review.
5. Preserve unrelated dirty worktree changes.

## Development loop

Use test-driven development at stable seams:

1. add the smallest failing unit/conformance test;
2. implement the narrow behavior;
3. run the focused test and typecheck/Clippy;
4. refactor without broadening the contract;
5. run component suites;
6. run the full workspace suite once at the end.

For TypeScript UI changes, run the exact package test file and `tsc --noEmit`
regularly. For Rust, prefer package/test-target selection while iterating.

## Compatibility changes

Changes to pack revisions, schemas, protocol, storage migrations, platforms,
tools, or release graph may require compatibility contract updates. Use:

```sh
cargo run --locked -p xtask -- compat verify
```

Follow the generator/check workflow printed by `xtask`; do not manually make
the TOML and JSON mirror disagree. Retained pack revisions and migration
checksums are immutable evidence.

## Documentation maintenance

- update the owning normative doc with behavior changes;
- update this manual when a developer workflow/capability changes;
- add a dated research note for time-sensitive provider/platform facts;
- label examples as offline, live, test-only, or design-only;
- verify external claims against first-party sources before public v0.1;
- never turn a future roadmap item into present-tense capability copy.

Manual commands:

```sh
pnpm docs:dev
pnpm docs:test
pnpm docs:lint
pnpm docs:build
pnpm docs:preview
```

## Review and commit

Review two axes separately:

- **Standards:** repository guidance, canonical vocabulary, security/privacy,
  and code-smell judgment.
- **Spec:** missing/partial requirements, wrong behavior, and scope creep.

Run `git diff --check`, inspect the exact staged file list, and never sweep
unrelated user changes into the commit. Commit messages should describe the
behavior and include relevant issue IDs when one exists.

Source: [domain contributor rules](https://github.com/imom39a/worldstream/blob/main/docs/agents/domain.md),
[gates](https://github.com/imom39a/worldstream/blob/main/docs/gates.md),
and [decision index](https://github.com/imom39a/worldstream/blob/main/docs/decision-index.md).
