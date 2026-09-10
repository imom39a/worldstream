# Heist prototype artwork

The production-facing shared renderer is `src/HeistArtwork.tsx`. It accepts
only the presentation slots `route`, `entry_window`, `required_tool`, and
`extraction`; labels are HTML beside the image. Unknown wire values render a
neutral placeholder and are never interpreted as private clues or outcomes.

The twelve bounded WebP assets (`quality 82`, max width 1200) are the browser
assets. The PNGs remain as provenance/source copies and are not imported by
the client. Entry-window art is mission-relative early/middle/late, not
timeofday. Its timeline dots are HTML, not painted into the image.

Created 2026-09-10 using the built-in image_gen tool (not the API/CLI fallback).
Nine separate image requests; no edits or regeneration. Originals remain in the
Codex generated-images directory. Project copies are the PNG files in this directory.
All nine images are presentation only: their appearance does not establish facts,
legality, equipment costs, hidden clues, or outcomes.

## Final prompt set

Each request used this exact shared prompt, followed by its Primary subject below.

Use case: stylized-concept. Asset type: original illustration for one selectable card in the Agent Heist cooperative planning game. Cohesive art direction: cinematic painted noir, tactile brushwork, deep midnight navy and desaturated blue-green shadows, restrained amber practical light, crisp readable central silhouette, atmospheric but not muddy. Landscape 3:2 composition, single subject taking most of the frame and safe for a centered square crop on phones. This is standalone in-game art, NOT a screenshot or card frame. No text, lettering, numbers, logos, watermark, UI, borders or card chrome. No people. Keep similar visual emphasis to other cards: no green correct-answer glow, no victory symbols.

### service-entrance.png

Primary subject: A discreet steel service entrance in a rain-darkened city building, illuminated by one warm light over the recessed door. Utility pipes, wet concrete, a small loading step. The door is the unmistakable focal object. No signs.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-aa465cef-70ff-4380-8036-715a8d81576e.png`

### thermal-key.png

Primary subject: A fictional compact thermal key tool resting in a fitted dark case: palm-sized brass and graphite handle, ceramic head with a short amber glowing heat element, elegant precise mechanical detail. It should look like a specialized key-shaped tool, not a gun. No schematic or instruction.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-bffeebca-5236-451c-b1da-c7892e0fbeac.png`

### boat.png

Primary subject: An unbranded small dark motorboat tied beside a quiet city dock at night. Three-quarter side view; the whole boat is visible and clearly recognizable. Gentle water reflections and a warm dock lamp.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-0dbb6c44-7f0e-4635-aa71-33fbdbc123e2.png`

### canal.png

Primary subject: A narrow urban canal access route beneath a low stone arch, water leading toward a recessed waterfront passage. Wet masonry, subtle teal water and warm tunnel light. No boats. The passage and waterway are the focal route.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-0ac4482d-e999-438f-8b9e-b3e96e6b65c2.png`

### rooftop.png

Primary subject: A city rooftop access route at night, a narrow roof walkway leading toward a clearly visible metal rooftop door and ladder, atmospheric skyline behind. Slate roof and warm light by the door. Clear architectural silhouette.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-677d4076-338b-46cf-9d8a-d0aedd3226d3.png`

### disguise.png

Primary subject: An anonymous folded charcoal work jacket with a matching cap and an entirely blank plain ID badge laid neatly on a dark equipment table. A wearable disguise kit, no person or mannequin, no readable badge details. Warm edge lighting.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-9da326d4-e54f-4dc5-94b9-5a879e1de5d9.png`

### jammer.png

Primary subject: A fictional compact graphite signal-jammer box resting on a dark equipment table, two short antennas, an unlettered amber indicator and knurled dial. Distinct from a key or weapon. Clean simple silhouette, no wiring diagram or functional instructions.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-d0e875e8-cb6e-4db5-8a48-19da4fc69641.png`

### van.png

Primary subject: An unbranded dark panel van parked alone in a wet narrow city side street at night, three-quarter front-side view with the whole vehicle visible. Restrained amber streetlamp reflection; clear practical getaway vehicle silhouette.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-32b752f8-dd59-4ad5-8f55-2253dcdd0211.png`

### motorbike.png

Primary subject: An unbranded dark city motorbike parked alone under a streetlamp on rain-wet pavement at night, three-quarter side view, entire two-wheeled silhouette clearly visible. No rider, no racing logos; restrained amber highlights.

Original: `/Users/vinothshanmugam/.codex/generated_images/01a081cc-b6c2-74e3-a08e-b008849a6b5e/exec-254436e2-ac00-4da7-b926-eff25093dd09.png`


### Added entry-window assets

`entry-early.png`, `entry-middle.png`, and `entry-late.png` are generated
mission-window illustrations. Their exact prompt/original paths are recorded
in the generated-image provenance sidecar maintained at the repository
workspace level; the client does not depend on that sidecar at runtime.

## Integration

The prototype uses these images in route/equipment/extraction choice cards, the
revealed Navigator dossier, shared clue thumbnail and sealed-plan review.
Timing uses a neutral three-segment early/middle/late diagram because the rules
supply no actual clock times. Labels and selection feedback are real HTML.
Every alternative is illustrated equally; no answer-specific highlight exists.
Verified the three route, equipment and escape images in the local browser;
equipment and escape images loaded successfully at phone size. The 390×844
plan screen has no horizontal or vertical page overflow and its primary action
is visible. Clicking an illustrated choice still updates the selected plan.
TypeScript compilation passed. This is prototype verification, not live acceptance.
The checked-in WebP derivatives are bounded responsive release assets and keep
the twelve-card set under the page-weight budget. The source PNGs remain for
review and provenance; changing source art requires regenerating all WebP
derivatives with the same dimensions/quality policy.
