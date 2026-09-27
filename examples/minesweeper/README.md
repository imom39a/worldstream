# Minesweeper comparison example

This standalone application compares an OpenRouter model with a TypeSafe JEV
participant on the same seeded board. It does not use the WorldStream kernel
or an Activity Pack. It remains as an application experiment.

## Run locally

Set `OPENROUTER_API_KEY` and `JEV_API_KEY` in the environment or an ignored
repository `.env` file. The server keeps these keys in its own process.

```sh
uv run --project sdk/python python -m examples.minesweeper.server
```

Open `http://127.0.0.1:5391/`. Select the models, board size, and mine count.
Select **Start two Rooms** to start provider calls. The default board has 8×8
cells, 10 mines, and a 300-second limit.

The `/v2/` view provides the large-board experiment. Run the module with `--help`
for host, port, and HTML path options. Provider integrations are experimental;
the repository does not maintain their current compatibility.

## Round processing

1. Both games start with the same seed.
2. The server derives constraints from the revealed numbers.
3. It removes proven mines from the offered cells.
4. If proven safe cells exist, it offers only those cells.
5. Otherwise, it offers candidate guesses with estimated mine risks.
6. Each engine receives the board summary and a bounded set of candidates.
7. The server checks the returned choice and applies it.
8. It sends the new public state and decision trace to the browser.

The first reveal is safe. A later mine ends the game. The code performs exact
deduction; providers select from the remaining candidates. The application has
no separate judge or repair turn.

After three failed provider attempts or an invalid choice, the game records a
`fallback`. It then reveals a deterministically selected hidden cell. Results
that include fallback moves do not measure only provider decisions.

Large boards use a bounded candidate set and local constraints. Boards with
more than 4,096 cells omit the full grid from provider input. The default
candidate limit is 20; the maximum is 48.

## HTTP routes

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/`, `/v2/` | Browser views |
| GET | `/config` | Provider catalog, defaults, and key availability |
| GET | `/events` | Public comparison state through server-sent events |
| GET | `/feed?side=left\|right` | Retained events for one game |
| POST | `/start` | Start the configured comparison |
| POST | `/stop` | Stop both games |
| POST | `/pause`, `/resume`, `/end` | Control one side |

## Tests and source

```sh
uv run --project sdk/python python -m pytest examples/minesweeper
```

Tests use provider substitutes and make no paid API calls.

- `game.py` contains board rules and candidate generation.
- `agents.py` contains provider clients.
- `server.py` contains HTTP routes and game orchestration.
- `web/` contains the static browser views.

The experiment does not establish that either provider is a better solver.
Board density, candidate selection, and fallback behavior affect the outcome.
