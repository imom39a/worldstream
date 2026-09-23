# Minesweeper: LLM vs JEV

A standalone two-room demo where an OpenRouter LLM and a TypeSafe JEV
participant each play the *same* seeded Minesweeper board, side by side. It is
the Minesweeper sibling of the Tower of Hanoi comparison: same room shell, same
public event feed, same decision-trace panel. There is no WorldStream Runtime,
no Pack, and no judge — JEV is a direct player.

## Why this works without history

A Tower of Hanoi decision depends on where the tower has been: the same board
can be reached by different move histories, and a looping Room needs cycle
hints. A Minesweeper decision does not. The current board is the entire input:
every number, every hidden cell, and the constraints around it are visible in
the authoritative state at this round. That makes a direct engine-vs-engine
comparison fair — both rooms receive the exact same `agent_state` and the exact
same closed candidate set, and neither engine needs to reconstruct prior state.

## Run it

The server keeps both provider keys in its own process. Put them in the
repository `.env` (or the environment):

```
OPENROUTER_API_KEY=...
JEV_API_KEY=...
```

The existing `openouterkey` in `.env` is also accepted as the OpenRouter key.

```sh
sdk/python/.venv/bin/python -m examples.minesweeper.server
```

Open <http://127.0.0.1:5391/>. Choose a model and a JEV model, set the board
size and mine count, and press **Start two Rooms**. The default is an 8×8 board
with 10 mines and a 300-second shared clock.

The JEV-only large-board view is at <http://127.0.0.1:5391/v2/> (see below).

Options: `--host`, `--port`, `--html`, `--v2-html`.

## How a round works

1. Both Rooms share one seed, so they start from the same board.
2. Before each round the server runs exact constraint propagation over the
   revealed numbers. Cells proven to be mines are removed from the offered set.
   When any cell is proven safe, only proven-safe cells are offered; otherwise
   every non-mine cell is offered as a guess, ordered by a computed mine-risk
   estimate.
3. Each round, the server builds one `agent_state` (grid, counts, candidates,
   `provably_safe`, `mine_risk`) and one closed candidate set, and hands both to
   each engine.
4. The left Room asks the OpenRouter LLM for one cell; the right Room asks JEV
   one closed `Choice` whose criteria are the offered cells.
5. The server validates the choice, applies it, and publishes a sanitized event
   and decision trace. The first reveal is always safe; a mine ends the Room.

Code owns the exact Minesweeper deduction; the engines judge the cells that
remain uncertain. There is no judge and no repair turn. If a provider fails
three bounded attempts, or returns a cell that was not offered, the Room records
a `fallback` and reveals a deterministic random hidden cell so the game cannot
stall.

## Density and comparison-design findings

A dense board is a guessing game, and the first LLM-vs-JEV runs made that
obvious: a 20x20 board with 99 mines lost every game, and both engines died on
the same cell. Measured with the code's own "reveal the lowest-risk offered
cell" policy over ten seeds per board:

| Board | Mine density | Games won | Avg provably-safe moves | Avg guesses |
| --- | --- | --- | --- | --- |
| 9x9 / 10 | 12.3% | 10/10 | 17.6 | 1.7 |
| 20x20 / 50 | 12.5% | 8/10 | 82.4 | 1.9 |
| 16x16 / 40 | 15.6% | 7/10 | 67.0 | 3.6 |
| 20x20 / 70 | 17.5% | 2/10 | 99.1 | 6.9 |
| 16x16 / 60 | 23.4% | 0/10 | 38.1 | 5.3 |
| 20x20 / 99 | 24.8% | 0/10 | 22.0 | 3.0 |

Two conclusions:

- **Density, not mine placement, is the dominant factor.** The board is valid
  at any density, but above roughly 16% mines the exact solver proves few cells
  safe and most moves become ~20-27% coin flips, so losing is near-certain.
  Keep demo boards at or below ~16% (for example 20x20 with 50-60 mines, or
  16x16 with 40).
- **The original prompt removed engine independence.** It told both engines to
  "choose the candidate with the lowest `mine_risk`", and the server had already
  sorted candidates by that same number. In every guess both LLM and JEV picked
  the same cell, so the comparison collapsed to "did the code heuristic guess
  right". JEV also reported ~98% confidence on a cell whose actual mine
  probability was ~20-25%. The v2 comparison removes the forced minimum and lets
  each player choose, and adds the subset deduction rule so more cells are
  proved safe before any guess.

## v2: JEV-only on a large board

`/v2/` runs one JEV player on boards up to **500x500** (default: expert, 30x16
with 99 mines). There is no second room and no LLM; the question is simply how
far JEV gets. Start it with `left_engine = "none"` and `right_engine = "jev"`.

Mines are capped only by board capacity (`width * height - 1`), so 200x200 can
hold 9000 mines and 500x500 up to 249999. The UI adjusts the input maximum as
you change the dimensions.

A large board would be unplayable if the state carried the whole grid and every
hidden cell, so the state is built to stay bounded:

- **Frontier only.** Candidate generation scans revealed numbers and their hidden
  neighbours, never the whole board, so a 500x500 board costs the same as a small
  one.
- **Bounded candidates.** At most `candidate_limit` cells (default 20, max 48)
  are offered per round, and cells proven to be mines are removed first.
- **No grid on large boards.** Boards above 4096 cells omit the full grid; the
  state carries the board summary and each candidate's compact local constraints
  (`r10c20=2(3h)`) instead.
- **Lean criteria.** Each JEV choice criterion is a short line such as
  `r11c23 · provably safe · r12c22=2(7h)`.
- **Lean streaming.** The board is sent as one flat token string, and SSE frames
  are only pushed when state changes (5s heartbeat), because the client computes
  the countdown locally.

Measured: a 120x120 board keeps the JEV state at ~2.8 KB and the offered set at
the configured limit; a 30x16 expert state with a frontier is ~4.8 KB including
the small grid.

## HTTP surface

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/` | The LLM-vs-JEV two-room UI |
| GET | `/v2/` | The computer-solver-vs-JEV UI (expert board) |
| GET | `/config` | Live cheap OpenRouter catalog, JEV catalog, defaults, key presence |
| GET | `/events` | Server-sent events carrying the sanitized comparison snapshot |
| GET | `/feed?side=left\|right` | Full retained event history for one Room |
| POST | `/start` | `{width, height, mines, duration_seconds, model, jev_model, left_engine?, right_engine?, seed?}` |
| POST | `/stop` | Stop both Rooms |
| POST | `/pause` / `/resume` | `{side}` freezes one Room's loop |
| POST | `/end` | `{side}` ends one Room |

## Tests

```sh
sdk/python/.venv/bin/python -m unittest \
  examples.minesweeper.test_game \
  examples.minesweeper.test_server
sdk/python/.venv/bin/python -m ruff check examples/minesweeper
```

The server tests drive the real run loop with mocked providers, so they never
call a paid API.

## Files

- `game.py` — pure board logic, candidate ordering, state/criteria builders
- `agents.py` — bounded OpenRouter and TypeSafe System One clients
- `server.py` — HTTP/SSE orchestration for both Rooms
- `test_game.py`, `test_server.py` — unit tests
- `web/demos/community-minesweeper-comparison.html` — the two-room UI
