# Project site

Plain HTML and CSS, with a local SVG favicon. No package install, generated
bundle, backend, analytics, or remote font is required.

From the repository root:

```sh
python3 -m http.server 8080 --bind 127.0.0.1 --directory site
```

Open `http://127.0.0.1:8080`. The GitHub Pages workflow uploads this directory
as-is after changes land on `main`. Set the repository's Pages source to
**GitHub Actions**. `node scripts/check-site.mjs` checks local assets, anchors,
and repository links without requiring a browser.
