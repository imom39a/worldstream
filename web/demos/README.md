# WorldStream technical demos

This workspace contains the public demo catalog and recorded browser demos. It does not contain a public WorldStream authority.

Follow `docs/demo-site-style.md` for layout, color, language, and accessibility rules. That document defines the shared visual system for both public sites.

## Local development

From the repository root:

```sh
pnpm demos:dev
```

Open `http://127.0.0.1:5180/`.

Public routes:

- `/` — filterable demo catalog;
- `/demos/agent-heist/` — recorded Agent Heist inspector; and
- `/docs/agent-heist/` — technical fixture documentation.

## Checks

```sh
pnpm demos:test
pnpm demos:lint
pnpm demos:build
```

## Vercel publication

Publication uses the local checkout. The Vercel project has no Git connection and no CI/CD workflow.

From `web/demos`:

```sh
vercel build --prod
vercel deploy --prebuilt --prod
```

Run these commands only after the local branch is reviewed and merged.
