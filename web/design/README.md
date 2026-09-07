# Shared visual foundation

`tokens.css` supplies the first-party platform, manual, fixture documentation,
and client foundation. It contains presentation only: no application state,
game rules, runtime dependencies, or authorization logic. Each Vite application
bundles imported assets independently.

Follow [`docs/demo-site-style.md`](../../docs/demo-site-style.md) for the shared
system and [`docs/activity-platform-design.md`](../../docs/activity-platform-design.md)
for individual Activity Client direction.

- `assets/rajdhani-bold.ttf`: Google Fonts Rajdhani Bold, licensed under the
  accompanying `rajdhani-OFL.txt`.
- `assets/worldstream-crossroads.webp`: the shared platform setting, generated
  with built-in imagegen. The original prompt is retained in
  [`web/demos/src/assets/README.md`](../demos/src/assets/README.md).

Retained, content-addressed releases preserve their original bytes. A design
change produces a new Activity Client Release; it never recolors an old one.

## Social preview artwork

The existing previews were refreshed with built-in imagegen on 2026-09-07,
using their prior raster image and the original crossroads artwork as references.
They are original decorative illustrations with no game-state meaning.

- `web/manual/public/og.png`: original retained at
  `/Users/vinothshanmugam/.codex/generated_images/01a07d1a-e004-7272-a8ce-1f31e2bd56ba/exec-5aa692bc-e5b9-490f-939d-ef903b45ffa0.png`.
  Brief: replace the purple technical diagram with deep teal, cream, and warm
  gold; crossroads on the right and quiet dark space on the left; serif text
  “WorldStream / Developer / Manual” and the subtitle “Build, operate, and extend
  WorldStream.” No browser frame, UI diagram, watermark, or additional copy.
- `web/demos/public/og.png`: original retained at
  `/Users/vinothshanmugam/.codex/generated_images/01a07d1a-e004-7272-a8ce-1f31e2bd56ba/exec-4b8fcb8c-f42b-44b6-a501-b28f25b53f79.png`.
  Brief: an illustrated multi-world crossroads on the right, quiet dark space on
  the left; “WORLDSTREAM”, cream serif “Different worlds.”, gold italic “Your next
  move.”, and “People. Agents. Play.” No technical diagram, browser frame, HUD,
  watermark, or additional copy. Both previews use a landscape composition with
  generous text margins.
