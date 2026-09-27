# Shared visual foundation

`tokens.css` supplies presentation tokens shared by the browser examples. Each
Vite application bundles its imported assets independently. These styles contain
no game rules, application state, or authority logic.

- `assets/rajdhani-bold.ttf`: Google Fonts Rajdhani Bold, licensed under the
  accompanying `rajdhani-OFL.txt`.
- `assets/worldstream-crossroads.webp`: original decorative artwork generated
  with imagegen for the experimental project. It has no game-state meaning.

A source change produces new build bytes and therefore a new client release
identity. Existing content-addressed declarations are retained test inputs.
