# WorldStream demo site style

Status: Maintained

This document governs the WorldStream demo catalog and its demo pages. New demo artifacts must use these rules unless an accepted ADR records a different decision.

## Purpose

The demo site is a technical interface. It shows observable WorldStream behavior. It is not a marketing site.

The visual reference is [Heroic Labs](https://heroiclabs.com/). Use its general layout discipline and visual proportions. Do not copy its assets, names, illustrations, page text, or page order.

The catalog interaction is based on the [Lightstreamer demo gallery](https://demos.lightstreamer.com/). Keep search, filters, clear demo categories, and direct demo entry points.

Use [ASD-STE100 Simplified Technical English, Issue 9](https://www.asd-ste100.org/assets/files/ASD-STE100_ISSUE9.pdf) as the writing basis. The current text is an STE-based draft. Do not claim formal compliance without a controlled dictionary and qualified human review.

## Layout system

Use this catalog sequence:

1. Compact primary header.
2. Centered technical introduction.
3. Full-width light catalog section.
4. Search and typed facet filters above an equal-width card grid.
5. Contrasting status band with factual system data.
6. Centered capability reference.
7. Small technical footer.

Use this demo-page sequence:

1. Compact primary header.
2. Demo title and explicit data mode.
3. Always-visible fixture, privacy, or connection notice.
4. Short guided checklist.
5. Factual status rail.
6. Interactive technical stage.
7. “What this demo shows” section and data-flow diagram.

Do not add a sales action to the header. Show private source status as plain text.

## Design tokens

Use CSS custom properties. The current base tokens are:

| Function | Token | Value |
| --- | --- | --- |
| Deep technical surface | `--purple-950` | `#292957` |
| Status and header surface | `--purple-800` | `#3e3d7e` |
| Primary control | `--purple-700` | `#504aa5` |
| Active accent | `--purple-600` | `#675ce7` |
| Light section | `--lavender-100` | `#f1f0fb` |
| Light border | `--lavender-300` | `#d6d2f0` |
| Status accent | `--turquoise-500` | `#00cfc5` |
| Dark text | `--slate-950` | `#20273a` |
| Body text | `--slate-600` | `#667085` |

Purple identifies WorldStream surfaces and controls. Turquoise identifies active or connected status. Amber identifies time or pending state. Do not use color as the only state indicator.

## Typography

- Use Inter or the system sans-serif fallback.
- Use a monospace font only for identifiers, sequence values, Semantic Time, and typed record names.
- Use normal interface sizes. Do not use very small decorative copy for required information.
- Use tight display lettering only for the page title.
- Use sentence case for headings and controls.
- Use uppercase only for short status labels.

## Components

### Header

- Keep the header compact.
- Include the WorldStream name, direct section links, and source status.
- Do not include a featured-demo, pricing, sign-up, contact, or sales control.

### Demo cards

Define each card in the typed demo manifest. Each entry must include a stable ID, title, summary, Activity Pack, capabilities, experience, perspectives, availability, thumbnail, route, documentation route, backend requirement, and optional build identity.

The catalog can filter these facets:

- Activity Pack;
- capability;
- experience;
- perspective;
- availability; and
- demo type.

Each card must state:

- what the demo contains;
- which capabilities it shows;
- whether data is recorded or live;
- whether the route is available; and
- whether a backend is required.

Use `Open recorded demo` only for an available fixture. Use `Live demo planned` when the required backend does not exist.
Provide a `How it works` link for each card. The link must resolve on the public demo site or in public technical documentation.

### Status surfaces

- Use factual labels and values.
- Derive live status from a bounded readiness check.
- Do not show a hard-coded live or healthy state for a network service.
- For recorded fixtures, label all health and lineage values as fixture data.
- Show an exact browser build revision and exact protocol identity where the values are relevant.

### Technical diagrams

- Build simple diagrams from CSS layout and text where possible.
- Label the source, authorization boundary, and visible output.
- Do not copy a reference-site illustration.
- Do not imply that illustrative hashes or lineage values are verified.
- State when the browser represents an authorization boundary but does not enforce it.

### Recorded evidence

- Read exact identity and Replay status from a checked-in evidence artifact when one exists.
- Name the evidence schema, Activity Pack version, and semantic revision digest.
- State when the browser does not execute the Activity Pack Revision or Replay.
- Do not call a Room sequence a Cursor.
- A Cursor is a Membership's acknowledged Observation Stream position.
- Do not call a mixed list of summaries an Observation Stream.
- Do not bundle credentials, capabilities, private user data, or live Room data in a public fixture.

## Language rules

Use the canonical vocabulary in `CONTEXT.md`.

- Use one term for one concept.
- Treat canonical WorldStream terms as approved project technical nouns.
- Use one subject per sentence.
- Use no more than 25 words in descriptive sentences.
- Use no more than 20 words in procedural sentences.
- Put one instruction in each procedural sentence.
- Use active voice when it is clear who performs the action.
- Use direct controls such as `Open recorded demo`, `Run`, `Step`, `Reset`, and `Close`.
- State whether data is recorded, simulated, illustrative, or live.
- State when no backend or network is connected.

Do not use slogans, metaphors, slang, superlatives, or unsupported claims. Avoid these patterns:

- “revolutionary”;
- “game-changing”;
- “seamless”;
- “best-in-class”;
- “unlock”;
- “powerful” without a measured definition;
- “production-ready” without acceptance evidence;
- “live” for recorded or simulated data; and
- “proof” when the screen contains illustrative fixture values.

## Accessibility

- Use landmarks and one level-one heading per document view.
- Provide a skip link.
- Give each control a visible label.
- Use `aria-pressed` for persistent toggle state.
- Put status updates in a polite live region when needed.
- Keep keyboard focus visible.
- Keep text contrast at WCAG AA or better.
- Make all controls at least 38 CSS pixels high where layout permits.
- Preserve usable content at 320 CSS pixels.
- Respect `prefers-reduced-motion`.
- Do not start fixture playback before the user selects `Run`.

## Social previews

- Use the site title and one factual sentence.
- Use the same purple, lavender, and turquoise system.
- Do not include customer marks, people, testimonials, or sales text.
- A detail page can omit an image when it has no record-specific image.
- Do not reuse the catalog image as if it were a detail-record image.

## Review checklist

Before publication, confirm all items:

- [ ] Layout follows the catalog or demo-page sequence in this document.
- [ ] Copy uses canonical WorldStream terms.
- [ ] Copy is an STE-based draft with no marketing claims.
- [ ] Data mode and backend state are explicit.
- [ ] Private source status is not an interactive control.
- [ ] Recorded data does not contain credentials or private user data.
- [ ] Public and Operator views do not receive participant-private values.
- [ ] Keyboard focus, narrow-screen layout, and reduced motion are usable.
- [ ] Root and representative detail metadata match their page content.
- [ ] Tests, type checking, and the deployment build pass.
