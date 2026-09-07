# WorldStream shared design guide

Status: Maintained · 2026-09-07

This is the shared visual foundation for the activity platform, developer
manual, fixture documentation, Inspector, and first-party Activity Clients.
It supersedes the former purple/lavender technical-site palette. Follow
[Activity Platform experience design](activity-platform-design.md) for the
platform hierarchy and individual pack settings.

## One family, distinct experiences

The platform is a collection of Activity Packs. Agent Heist is one pack;
its story, terminology, imagery, and gameplay must not define the whole site.

Use the illustrated sense of place of [AI Dungeon](https://aidungeon.com/)
and the strong game identity of [Shards](https://play-shards.com/) as references.
Use original artwork and compositions.

- Discovery: an illustrated crossroads, warm serif headings, distinct pack
  covers, and direct activity entry.
- Developer manual: the same identity, colors, and controls, with quiet reading
  surfaces, persistent search, clear procedures, and readable code.
- Fixture documentation: the same reading treatment as the manual, with explicit
  recorded-data boundaries and complete evidence details.
- Inspector: the shared technical palette and controls, with a neutral identity
  and no pack-specific rules or imagery.
- Agent Heist: rainy city artwork, condensed tactical titles, role panels,
  amber decisions, and cyan intelligence accents.
- Negotiate: walnut and burgundy surfaces, copper highlights, serif headings,
  and a paper treatment for the exact agreement bytes.

The design must not add gameplay, a universal score, fictional activities,
unsupported availability, or hidden information.

## Canonical tokens

Import [`web/design/tokens.css`](../web/design/tokens.css). Do not copy a
second set of shared color constants into new applications. Activity Clients
may define additional pack-specific colors and illustration treatments.
Imports are build-time presentation only; each application bundles its own
assets and executes independently.

| Function | Token | Value |
| --- | --- | --- |
| Background | `--ws-background` | `#0b1415` |
| Surface | `--ws-surface` | `#111e20` |
| Raised surface | `--ws-raised` | `#1a2b2c` |
| Border | `--ws-line` | `#334542` |
| Primary text | `--ws-text` | `#f5f0e3` |
| Secondary text | `--ws-muted` | `#b3c2bc` |
| Primary action / focus | `--ws-gold` | `#f3c777` |
| Informational accent | `--ws-cyan` | `#79d9cc` |
| Error | `--ws-danger` | `#ffb3ab` |
| Code background | `--ws-code` | `#081113` |

Use gold controls with dark text. Use cyan sparingly for information or
confirmed connection status. Pending and unavailable states need text labels;
color never establishes availability or replaces a status label.

## Typography and spacing

- Use `--ws-title-font` for platform and documentation titles: Georgia with a
  serif fallback. Reserve large titles for the page introduction.
- Use self-hosted Rajdhani through `--ws-ui-font` for the brand and tactical
  Heist headings. License and asset provenance live in `web/design/README.md`.
- Use `--ws-body-font` for reading and controls. Keep ordinary text at least
  14–16px where possible, with generous line height.
- Use `--ws-mono-font` for code, exact identities, and times. Give these values
  room to wrap or scroll without overlapping nearby controls.
- Use restrained borders and small corner radii. Artwork belongs in discovery
  and activity environments; code and long documents need quiet backgrounds.

## Manual and documentation

Keep search available on every manual page. Use a dark sidebar with a gold
active rail, one page title, and a clear reading column. Keep related activity
navigation available on phones. A closed mobile menu must not remain reachable
by keyboard. Escape closes search results and the mobile menu.

Guides, capability filters, search results, code blocks, tables, callouts,
copy buttons, and empty states all use the shared tokens. Show factual status
labels for implemented, reference, design-only, and deferred capabilities.

Keep technical language precise and use `CONTEXT.md` vocabulary. Write direct
procedures, one instruction per sentence. Playful discovery copy is appropriate
for players; operational documentation must remain literal and accurate.

## Activity entry and results

Derive entry availability, seats, public viewing, and House Agents from the
reviewed listing and service response. Do not add inactive search or fake
activity cards to fill a catalog. Unknown packs receive a neutral treatment.

Keep authentication, role selection, invitations, waiting-room feedback,
connection recovery, result pages, and errors within the shared visual family.
Technical identities may sit in disclosures when they do not help the next
decision. Do not hide meaningful warnings or evidence.

Results remain activity-specific. Agent Heist results do not constitute a
cross-activity leaderboard.

## Recorded data and authority

- State whether a view is recorded, illustrative, disconnected, or live.
- Read fixture identity and Replay status from checked-in evidence.
- State when the browser does not execute the Activity Pack or Replay.
- A Room sequence is not a Cursor. A mixed summary list is not an Observation
  Stream. Use those terms accurately.
- Never substitute recorded data for a failed live connection.
- Public and Operator views must not expose participant-private values.
- Keep credentials, live Room data, and private user data out of public assets.
- Simple technical diagrams should use HTML/CSS or existing code assets and
  label the source, authorization boundary, and visible output.

## Metadata and images

Use the same shared palette for browser theme colors and existing social
preview images. Titles and descriptions must describe the page actually served.
A platform preview must represent the collection, not one Activity Pack.
Do not reuse a platform image as if it showed an individual result.

Bundle artwork locally. Record generated-asset prompts and provenance. Preserve
font licenses. Do not depend on a reference game's image server.

## Accessibility

- Use landmarks, one level-one heading, visible labels, and skip navigation.
- Keep a visible gold keyboard focus ring. Use semantic buttons and links.
- Controls should be at least 44px high; dense documentation navigation can use
  36–38px rows while retaining clear focus and spacing.
- Meet WCAG AA contrast and keep tables, code, dialogs, and navigation usable
  at a 320px viewport without page-level horizontal overflow.
- Honor reduced motion. Start recorded playback only when the user selects Run.
- Restore focus after dialogs close. Announce meaningful status updates politely.

## Artifact and release coverage

Review the manual home, an article, capability search/filtering, mobile menu,
platform discovery, entry states, fixture docs, recorded Heist, live-client
boundaries, Inspector, and Negotiate. Include browser metadata, existing preview
images, design documentation, generated builds, and deployment configuration.

Immutable Activity Client Releases, old distributions, and their retained build
artifacts preserve their original bytes and identities. Styling changes create
new release and evidence files. Update current bindings and build/install
references coherently; never disable a digest check to publish a new design.

## Verification before source control

Run the affected application tests and type checks, documentation verification,
frontend builds, and exact client artifact/reference checks. Inspect desktop and
320px layouts, search, navigation, copy controls, and entry states. Commit the
complete source, original assets, licenses, updated guide, and versioned release
artifacts together. Generated disposable `dist` trees stay untracked.
