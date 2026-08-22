# macOS source quickstart parent verification

Date: 2026-08-21
Repository: `/Users/vinothshanmugam/code/agent-streamer`
Evidence class: disposable local source verification; not release evidence

## Command

```sh
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  bash scripts/macos-source-quickstart.sh
```

## Result

- APFS check passed on Darwin.
- Locked workspace build passed.
- Python SDK: 64 tests passed.
- Frozen pnpm install passed.
- Console: 40 tests passed and production build passed.
- Compatibility manifest verification passed with `manifest_kind=specification`
  and `release_ready=false`.
- The script explicitly produced no signed, notarized, or native binary release
  artifact.

This run strengthens the local macOS source handoff only. It does not resolve
the release-gated `macos-source-quickstart` evidence row or any artifact
identity in `compatibility.toml`.
