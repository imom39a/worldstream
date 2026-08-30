# A202 independent-peer interoperability

WorldStream's pinned A202 operated single-session profile has two different
proof obligations. The local adapter tests prove the implementation accepts
and rejects the frozen exact-byte contract. IMO-120 additionally requires an
independently implemented peer, built without WorldStream's serializer, to
exchange the same profile. The first obligation cannot stand in for the
second.

## Release invitation

`interop/a202-peer-v1/exchange.json` is a canonical, language-neutral
`worldstream/a202-peer-exchange-invitation/v1` document. It contains:

- the exact pinned revision, object/rules versions, scopes, and calibration
  profile;
- exact signed Offer and ActionEnvelope bytes as unpadded Base64url, their
  SHA-256 release-carrier identities and BLAKE3 wire identities, the exact
  media type, and a test-only public verification key;
- independently comparable canonical content bytes and signature-message
  bytes, so a peer does not need WorldStream's serializer to locate the
  signature boundary;
- `verified`, `failed`, and `not_checkable` expectations for valid, tampered,
  missing-key, and wrong-media inputs; and
- initial Room/A202 heads plus Room-only, transaction-head, and session-head
  retry cases.

The signatures are static test material with no product authority. The
invitation declares `evidence_class: specification_invitation` and
`release_evidence: false`. Its Rust test consumes the checked-in JSON as an
opaque external format and verifies all exact hashes, signature messages,
adapter outcomes, and retry decisions. That test is local specification
evidence only.

## Frozen exchange semantics

The peer sends the decoded exact bytes with
`application/a202-commercial+json`. The receiving side retains and hashes
those exact bytes; a parsed JSON value is only a view. A202 content hashing and
signature construction remove exactly the three top-level members
`content_hash`, `signatures`, and `kernel_annotations`. Nested members with the
same names remain covered. The signature message is the canonical content
bytes, one ASCII period, then canonical protected metadata containing
`algorithm`, `key_id`, `purpose`, and `signed_at`.

The signed ActionEnvelope owns A202's `expected_sequence`. WorldStream's Room
Head is a separate unsigned concurrency witness. If only that Room witness
changes, retry reuses the identical signed bytes. If either logical A202 Head
changes, the peer must rebuild and re-sign; WorldStream never patches the
existing object.

## External receipt boundary

The released kit's standard-library Python validator has two commands:

```sh
python3 validate-receipt.py validate-invitation --invitation exchange.json
python3 validate-receipt.py validate-receipt \
  --invitation exchange.json \
  --receipt peer-receipt.json
```

The peer receipt binds the invitation SHA-256, the exact independent
implementation artifact SHA-256, a pseudonymous participant, canonical timing,
closed independence attestations, and the exact sorted case results. The
validator rejects duplicate JSON keys, links/special files, oversize input,
noncanonical Base64url, profile/case drift, media/byte substitution, shared
WorldStream serializers or source, missing cases, and altered tri-state or
retry results.

A valid output is deliberately
`status: valid_external_receipt_candidate`,
`independent_evidence_established: false`, and `release_evidence: false`.
Schema validation cannot prove who ran the implementation. A maintainer must
review external origin, non-contributor status, independent control, and the
signed release-artifact provenance before a returned receipt may enter the
detached evidence process. Until that real run exists, IMO-120 stays In
Progress and public compatibility text must not claim independent A202
interoperability or conformance.
