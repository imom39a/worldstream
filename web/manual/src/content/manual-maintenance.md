# Maintain and publish this manual

The developer manual is a static React/Vite application in `web/manual`. Its
technical claims live in version-controlled Markdown; its navigation and
capability catalog are typed source. It has no runtime database, analytics, or
external font dependency.

## Run it locally

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm docs:dev
```

Open `http://127.0.0.1:4173`. The development server reloads Markdown, React,
and CSS changes.

## Content map

| Change | Edit |
| --- | --- |
| prose, commands, tables, source links | `web/manual/src/content/*.md` |
| page title, route, group, or summary | `web/manual/src/pages.ts` |
| capability catalog and status | `web/manual/src/capabilities.ts` |
| navigation/search/application behavior | `web/manual/src/App.tsx` |
| visual design | `web/manual/src/styles.css` |
| social preview | `web/manual/public/og.png` |
| GitHub Pages workflow | `.github/workflows/deploy-developer-manual.yml` |
| GitLab Pages pipeline | `.gitlab-ci.yml` |

Every capability must say whether it is **Implemented**, a **Reference**
integration/example, **Design only**, or **Deferred**. Do not turn a proposed
workflow or structurally compatible provider into a certified feature.

## Verify a change

```sh
pnpm docs:test
pnpm docs:lint
pnpm docs:build
pnpm docs:verify
```

The build writes `web/manual/dist`. The link verifier checks that every route
and local documentation link resolves before publication.

For technical claims, prefer the repository's contracts, ADRs, schemas, CLI
help, and tests. Use the provider's official documentation for external
integration syntax. Record compatibility gaps directly in the page instead of
guessing.

## GitHub Pages

The public developer manual is published at
[imom39a.github.io/worldstream](https://imom39a.github.io/worldstream/).
The checked-in GitHub Actions workflow verifies and builds the manual after a
relevant change lands on `main`, then deploys the resulting static artifact to
the `github-pages` environment. `actions/configure-pages` supplies the actual
site base URL and project subpath, so the Vite asset paths and social metadata
stay correct if the repository or Pages configuration changes.

The workflow can also be run manually from **Actions → Deploy developer manual
→ Run workflow**. Treat a successful build job as artifact evidence only; the
separate deploy job and the live URL are the publication evidence.

See [Using custom workflows with GitHub Pages](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages)
and [Configuring a publishing source](https://docs.github.com/en/pages/getting-started-with-github-pages/configuring-a-publishing-source-for-your-github-pages-site).

## GitLab Pages alternative

The checked-in pipeline publishes the static build only from GitLab's default
branch. It supplies the project subpath as `DOCS_BASE` and the final Pages URL
as `DOCS_SITE_URL`, so assets, navigation, and social metadata work whether the
site is hosted at a root domain or a project path.

For the current small-circle developer phase, keep the GitLab project private
and enable **Pages Access Control** in **Settings → General → Visibility,
project features, permissions → Pages** before sharing the URL. Access control
is a GitLab project setting, not a value this repository can safely force in
CI. Test the final URL both while signed in and in a signed-out browser; the
latter must not reveal the manual. See [GitLab Pages access control](https://docs.gitlab.com/user/project/pages/pages_access_control/).

A Pages-ready pipeline is not proof of deployment. Before calling either host
live, verify all of the following in the target project:

1. the remote points at the intended project;
2. `main` (or the configured default branch) contains this site;
3. the verification/build and Pages deployment jobs succeed;
4. the provider reports the expected Pages URL;
5. that URL returns the current commit's manual and social card.
6. repository and Pages visibility match the intended audience.

## Documentation release checklist

- run the full repository verification suite once;
- confirm commands against current `--help` output;
- verify privacy-safe examples contain no real authority material;
- inspect the built site at desktop and narrow widths;
- keep local-v0.1 limitations visible;
- commit research evidence with the documentation it informed;
- record the deployed commit and URL when Pages is actually enabled.

Additional GitLab sources: [GitLab Pages](https://docs.gitlab.com/user/project/pages/)
and [GitLab CI/CD YAML](https://docs.gitlab.com/ci/yaml/).
