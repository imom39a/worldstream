# Action starvation probe

`scripts/action-starvation-probe.py` measures the current strict Action
contract under a deterministic delayed Runner. It uses integer virtual time
and periodic Room updates, so the same matrix produces byte-identical JSON on
every run and requires no provider or network.

Run it with:

```sh
python3 scripts/action-starvation-probe.py --output /tmp/action-starvation.json
```

The report schema is `worldstream/action-starvation-probe/v1`. Each scenario
varies decision delay, Room update rate, participant count, update visibility,
and whether updates are related to the proposed work. It records accepted
useful Actions, exact stale rejections, useful-action latency percentiles,
model-equivalent wasted decisions, re-evaluations, update categories, and
bounded pending attempts. The sample trace retains only redacted sequence and
outcome metadata; it contains no private payloads or identities.

The adapter applies the production rule exactly: `based_on_room_seq` must equal
the Complete Head sequence at admission. The rule is mirrored from
`crates/worldstream-core/src/trace.rs::assess_stable_action_disposition`; the
probe does not invoke a live backend commit. A stale proposal is discarded.
The Runner refreshes, re-evaluates, and creates a new Action identity; the
probe never auto-rebases a stale Action.

`during_reasoning` starts with a synchronized Head and measures genuine Room
updates during the decision delay. `hidden_head_lag` is a separate IMO-217
baseline: a hidden update commits before the decision begins, but the Runner's
current view remains behind. Both cases can end in a stale rejection, while
the separate mode and counters show why they happened. Visibility labels
classify whether an update could have been delivered; they do not disclose
its private facts.

The initial decision envelope is deliberately narrow: decisions up to 250 ms
with at most two Room updates per second must eventually produce useful work
within eight attempts. The generated matrix passes that envelope and shows
starvation outside it when every retry races another whole-Head update. This
supports the current contract within the stated envelope and provides
measurement evidence for a future versioned successor ADR only if a broader
envelope is required. Participant count is included as a controlled occupancy
dimension while the aggregate Room update rate stays fixed. This probe
measures one target Runner's pending work and does not claim scheduler fairness
across participants or backoff behavior. It also does not claim production
commit throughput, storage, network, or model-quality results; IMO-217's live
synchronization implementation remains a separate concern.
