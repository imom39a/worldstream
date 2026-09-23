# Retained failure diagnosis

The measured attempt was stopped and cleaned up before this read-only analysis.
No Action was retried and no Room state was changed during diagnosis.

## Established facts

- The final coordinator outcome is `submission_uncertain` for Action
  `1XAQD9FPQET34QDEEF7MKFT7P5`.
- The Action carries the exact 4,826-byte baseline artifact and an empty
  `contribution_refs` list. The Pack's `submit_candidate` schema requires at
  least one entry (`packs/agent-swarm/src/schemas.ts`).
- The retained Room database ends at sequence 10. Its committed transitions
  and semantic receipts contain no record for that Action. There are no
  Candidates, Checks, Reviews, writebacks or Results.
- `AuthenticatedLocalControlPlane::act` maps gateway errors to
  `BackendError::ActionUncertain`. The retained coordinator outcome does not
  preserve a specific gateway response that establishes a schema rejection.
- Correction: the first integration response's `expected_digest` used the
  `sha256:` prefix. The field is supported, but accepts only a canonical
  `blake3:` digest; the coordinator rejected the format locally as
  `provider_output_invalid`. A later response removed the field. That correction
  did not establish the failure-led source-repair behavior requested by this
  experiment.

## Alternative checked

The gateway caps each received WebSocket text at 65,536 bytes. The largest
retained observation frame payload was 18,851 bytes; the latest activity
materialization was 18,723 bytes, and the complete Candidate Action JSON was
905 bytes. These retained sizes do not support an oversized-frame explanation.

The malformed Contribution-reference list is a concrete defect and a plausible
cause of the submission failure. Without a retained rejection response, that
specific causal explanation remains an inference. An absent committed Action
does not justify silently relabeling the coordinator's outcome as a received
rejection or automatically replaying an effect.

## Next reproducible check

Use an isolated regression at the production Action boundary: compile a
`submit_candidate` proposal with empty references against its exact offered
schema and establish a definite pre-submission validation result. Verify that
the model receives a bounded field-level explanation and can choose a revised
plan, while uncertain network outcomes continue to require reconciliation.
Do not weaken the Pack provenance rule or insert fabricated references.

Separately evaluate model dispatch against the accepted goal's temporal
requirements. The retained planning decision explicitly dispatched repair and
test authoring in parallel before any baseline Check, despite the repair Work
Item's stated prerequisite. This is a planning-quality failure independent of
the transport diagnosis.
