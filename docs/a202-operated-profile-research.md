# A202 operated-profile research

Date: 2026-08-30

## Question and pinned source

This note verifies the external contract behind the **WorldStream A202 v0.1
Operated Single-Session Formation Compatibility Profile**. The source under
review is the A202 repository at the exact immutable revision
[`fa85aa8b49bfe7b3f7ded487c98500a600e92e41`](https://github.com/a202-protocol/a202/tree/fa85aa8b49bfe7b3f7ded487c98500a600e92e41).
No claim below is based on the moving default branch.

The revision is later than the repository's `v0.1.0` release tag and is not
itself tagged. It identifies `a202-commercial/0.1`, but WorldStream must keep
the full Git revision in every compatibility statement and proof package; the
short protocol version alone does not identify these exact source bytes.

## Findings that define the adapter

1. **A202 leaves room for an operated venue.** The canonical model permits an
   operator to hold authoritative ordering while authorized parties retain
   independently verifiable records. The A202 reference implementation is
   explicitly not an operator: it has no venue, session-ordering service,
   network client/server, or storage. That is the narrow gap WorldStream fills;
   it is not a reason to create a new commerce protocol.
   Sources: [canonical model, storage boundary](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#12-storage-and-source-of-truth-boundary),
   [reference implementation boundary](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/reference/README.md#what-this-is-not).

2. **The transport object is the exact opaque byte sequence.** The A2A and
   plain-HTTPS binding uses media type `application/a202-commercial+json` and
   requires byte-identical RFC 8785 canonical JSON. A receiver verifies the
   received bytes; parsing and reserializing first verifies the receiver's
   serializer instead of the sender's object. Carrier framing stays outside
   signed bytes.
   Source: [A202 carrier binding, object transport](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/bindings/a2a-binding-v0.1.md#5-object-transport).

3. **A202 content hashes and signatures have their own meaning.** An object's
   SHA-256 excludes only top-level `content_hash`, `signatures`, and
   `kernel_annotations`. An ES256 signature covers those canonical content
   bytes, a literal `.` byte, and canonical JSON for its protected
   `algorithm`, `key_id`, `purpose`, and `signed_at` fields. Key status must be
   resolved both at signing time and verification time.
   Sources: [canonical model section 4](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#4-canonicalization-and-signatures),
   [informative reference signer](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/reference/a202_reference/signing.py).
   WorldStream's BLAKE3 wire digest is an additional exact-byte identity, not
   an A202 signature and not a replacement for A202 SHA-256.

4. **A202 and WorldStream concurrency witnesses must remain separate.** A202
   has independent logical transaction and session streams. Operated appends
   use the expected sequence of the targeted stream, while cross-stream
   sequence continuity is forbidden. A WorldStream Room Head orders all Room
   Transitions and therefore belongs outside the signed A202 object. A retry
   caused only by a changed Room Head may reuse the exact signed bytes when
   both A202 predecessor heads remain current; an A202-head change requires a
   new object and signature.
   Sources: [two levels of A202 state](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#2-two-levels-of-state),
   [per-stream concurrency](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#8-concurrency-and-isolation).

5. **WorldStream's selected operated surface is deliberately smaller than an
   A202 scope.** The upstream operated scope includes invitation onboarding,
   operator key custody, multiple session streams, isolation among rivals,
   awards, determinations, publication, and qualification. WorldStream imports
   an already-qualified opening history and operates exactly one bilateral
   session; it does not implement or claim the whole scope.
   Source: [A202 operated scope](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/conformance-role-scopes-v0.1.md#5-the-operated-scope).

6. **Successful formation has several distinct acts.** A current signed Offer,
   exact-hash Acceptance, any required exact Approval, operated selection, two
   party signatures over the same Agreement, and `agreement.committed` remain
   separate. Acceptance moves the session to `accepted`; it does not commit
   the transaction. The aggregate reaches `committed` only through the final
   event.
   Sources: [agreement formation](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#10-agreement-formation),
   [session-to-aggregate relationship](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#62-relationship-to-the-aggregate).

7. **Resolver unavailability is not permission.** Mandate evaluation resolves
   the signed mandate, delegation chain, status endpoint, validity, scope,
   constraints, approval rules, and stream guard in a stable order. An
   unresolved status denies a live act. Status responses are retrieved over
   HTTPS and have a pilot cache expiry no longer than 60 seconds. WorldStream
   records the exact response plus authenticated-source evidence so replay
   does not contact the network.
   Source: [commercial mandate evaluation and status](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/authority/commercial-mandate-v0.1.md#6-evaluation-algorithm).

8. **Evidence reports are per-check and three-valued.** The verifier executes
   canonical hash, signature, version-chain, stream-continuity, guarded replay,
   determination, and stated-gap steps. Each check is `verified`, `failed`, or
   `not_checkable`; gaps cannot be collapsed into either other result and no
   overall boolean may replace the checks.
   Source: [A202 evidence verification](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/evidence/evidence-verification-v0.1.md#4-the-verification-procedure).

## Pinned identities

| Identity | Pinned value |
|---|---|
| Repository revision | `fa85aa8b49bfe7b3f7ded487c98500a600e92e41` |
| Shared object model | `a202-commercial/0.1` |
| State-machine rules | `1.3` |
| Operated role-scope reference | `a202-scope/operated/0.1` |
| Required bilateral behavior reference | `a202-scope/bilateral/0.1` |
| Transaction profile | `a202-profile/calibration-service/0.1` |
| Commercial-kernel schema SHA-256 | `b4ca6b12a05590815e08d950d807620d722f14fd620dddd7ef5217fb63978de6` |
| Commercial-mandate schema SHA-256 | `b9111785deedcc9424a04e9d9b4d410fe026e3fdef45f5cfdf59acb62ce29327` |
| Calibration profile schema SHA-256 | `9740a1e4aa40aac1fee2da32ce8bf283e645b757480911f0c0bae8b42be156a7` |
| Conformance manifest SHA-256 | `5eb13df10f4f1222cac409659afb85a5586db8321bfb395dd14f4d973647b0c7` |

The schema and manifest values are copied from the pinned
[`schemas/digests-v0.1.json`](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/digests-v0.1.json),
which defines them over whole file bytes.

## Compatibility matrix

| Surface | WorldStream v0.1 |
|---|---|
| One imported, already-qualified opening history | Supported |
| One transaction, one operated bilateral session, two organizations | Supported |
| Exact Offer/counter, withdrawal, Acceptance, selection, Agreement signatures, commitment | Supported |
| Exact Human Approval of buyer Acceptance | Supported |
| Signed deadline resolution and expiry | Supported |
| Exact opaque bytes, A202 SHA-256, ES256 purpose verification, BLAKE3 wire identity | Supported by adapter/verifier |
| Authenticated resolver response capture with deterministic semantic binding | Supported by adapter/verifier |
| Cross-indexed party/protocol and venue/runtime proof | Supported by export/verifier |
| Publication, discovery, invitations, qualification performed by WorldStream | Not supported; opening history is imported |
| Auctions, rival sessions, competitive award | Not supported |
| Operator custody of commercial-party keys | Not supported |
| Obligations, performance, settlement, disputes, amendments, termination | Not supported |
| A202 conformance grade or certification | Not claimed |

## Verification performed and remaining interoperability limit

The exact pinned checkout's 34 isolated Python reference tests passed on
2026-08-30. That run transitively executed the public conformance runner and a
fixture sweep; the sweep exercised 21 of 40 evidence bundles and explicitly
reported 19 as outside the seven-step reference verifier. This is useful
upstream consistency evidence, not proof that WorldStream conforms.

The pinned schemas explicitly state that v0.1 publishes no RFC 8785 vectors,
signature vectors, or runtime fixtures. WorldStream therefore tests its Rust
adapter and separately implemented offline serializer against independently
constructed ES256 objects and pinned upstream shapes, but a real exchange with
an external A202 implementation is still required before any stronger
interoperability statement. If that exchange disagrees on exact signed bytes,
WorldStream must narrow or remove its compatibility statement; it must not
alter the Room Kernel to force agreement.
