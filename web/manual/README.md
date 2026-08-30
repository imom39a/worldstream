# WorldStream developer manual

This workspace contains the public developer manual.

Follow `docs/demo-site-style.md` for layout, color, language, and accessibility rules. That document defines the shared visual system for both public sites.

## Local development

From the repository root:

```sh
pnpm docs:dev
```

Open `http://127.0.0.1:4173/`.

## Checks

```sh
pnpm docs:test
pnpm docs:lint
pnpm docs:verify
pnpm docs:build
```

## Vercel publication

Publication uses the local checkout. The Vercel project has no Git connection and no CI/CD workflow.

From `web/manual`:

```sh
vercel build --prod
vercel deploy --prebuilt --prod
```

Run these commands after the local branch is reviewed and merged.
