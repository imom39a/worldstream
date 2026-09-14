# Runner context strategy evidence

This local, model-backed experiment compares the three Runner context forms
named by IMO-232 under the same model and declared prompt budget. It measures
policy continuation quality only. It does not submit Actions, qualify runtime
integrity, or constitute release evidence.

## Reproduction identity

- Harness commits: `e4562e4a`, `e6d87f12`, `39197aa0`
- Command: `uv run --python 3.14.7 --no-project python scripts/runner-context-evaluation.py --model gpt-5.6-luna --reasoning low --output docs/evidence/runner-context-evaluation/luna-low.json`
- Model: `gpt-5.6-luna`
- Reasoning effort: `low`
- Prompt ceiling for every strategy: 24,576 bytes
- Artifact: `luna-low.json`
- Artifact SHA-256: `afcf043c12e45a86957bb61172b9afff412c71f409474e812d2bf0e3c07c6be5`
- `release_evidence=false`

The harness ran each strategy in a separate ephemeral Codex invocation. Each
invocation received the same rule brief, output schema, four independent held
out cases, and scoring categories. The oracle remained in the harness and was
not included in the prompt. The cases cover an offline source revision, two
simultaneous obligations, conflicting superseded evidence, and a resolved item
with no current Action Offer.

## Result

| Strategy | Prompt bytes | Useful / expected | Missed work | Obsolete claims | Unsupported completion | Evidence errors | Safe abstentions |
|---|---:|---:|---:|---:|---:|---:|---:|
| Recent history | 3,364 | 2 / 4 | 2 | 0 | 0 | 0 | 1 |
| Summary plus authorized retrieval | 3,210 | 4 / 4 | 0 | 0 | 0 | 0 | 1 |
| Current Projection plus explicit work | 3,095 | 4 / 4 | 0 | 0 | 0 | 0 | 1 |

The recent-history context correctly abstained when it lacked an explicit
current offer, but it missed that obligation and one older obligation outside
the retained window. Both strategies carrying the complete current work set
produced all four exact typed assessment Actions and respected the no-offer
case. The summary result depended on its retrieved, versioned evidence; the
fallible prose summary was not scored as authority.

This is a small bounded experiment, so it supports the context-design choice
and closes the missing model-backed comparison row, not a general claim about
model accuracy. Raw responses, prompt hashes, elapsed times, reported token
usage, and per-case scores remain in the JSON artifact so failures cannot be
collapsed into a success count.
