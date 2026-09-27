# A202 independent-peer exchange kit v1

This directory is a language-neutral invitation for an independently built
A202 peer. It is a specification fixture, not a peer, an external run, a
conformance result, or release evidence. The locally generated signatures and
expected results prove only that the invitation is internally coherent.

An official release job must carry this complete directory inside the signed
`worldstream-a202-adapter` release subject. Send an adopter the extracted
release-subject directory and its verified release-manifest identity, never a
working-tree path. A run from this checkout is a maintainer dry run and cannot
satisfy IMO-120.

## What an independent peer implements

Use any language and any JSON, SHA-256, BLAKE3, Base64url, and ES256 libraries.
Do not link the WorldStream Rust adapter and do not copy its serializer.

1. Run `validate-receipt.py validate-invitation --invitation exchange.json`.
   This checks strict/canonical JSON, the closed profile and case inventory,
   Base64url form, exact-byte SHA-256 bindings, and all bounded fields. It does
   not implement the peer for you.
2. Decode `exact_bytes_base64url` and retain those bytes unchanged. The HTTP or
   message content type is the case's `input.media_type`; never parse and
   reserialize the object for transmission.
3. Independently recompute the canonical content bytes by removing only the
   top-level `content_hash`, `signatures`, and `kernel_annotations` members and
   applying the pinned RFC 8785-compatible integer-only subset. Compare your
   result with `reference_material` before verifying the fixed-width 64-byte
   ES256 signature.
4. Run every case. Report each check as exactly `verified`, `failed`, or
   `not_checkable`. Missing trust is `not_checkable`, not success and not a
   cryptographic failure.
5. Keep the WorldStream Room Head outside the signed A202 bytes. A Room-only
   Head change reuses the exact ActionEnvelope bytes. A transaction or session
   A202 Head change requires a new object and signature.
6. Return one strict `worldstream/a202-peer-exchange-receipt/v1` document with
   the exact sorted case inventory. `receipt.template.json` is deliberately an
   incomplete, non-receipt schema; it cannot be submitted as evidence.

For each invitation case, the returned `cases` array uses this exact shape and
copies the independently observed values, not a WorldStream-generated row:

```json
{
  "case_id": "offer_valid",
  "fresh_signature_required": false,
  "outcomes": {
    "a202_content_hash": "verified",
    "exact_byte_digest": "verified",
    "logical_head": "not_checkable",
    "media_type": "verified",
    "signature": "verified",
    "signer_status": "verified"
  },
  "received_exact_bytes_sha256": "sha256:<64 lowercase hex>",
  "received_media_type": "application/a202-commercial+json",
  "retry_decision": "not_applicable",
  "returned_exact_bytes_sha256": null
}
```

Sort rows by `case_id`. The validator requires the exact invitation result for
all eight rows and rejects any extra field or omitted case.

The independent implementation supplies its own artifact SHA-256 and a
pseudonymous participant ID. The receipt contains no URLs, filesystem paths,
credentials, private keys, raw logs, prompts, or commercial data.

## Validate a returned candidate

```sh
python3 validate-receipt.py validate-receipt \
  --invitation exchange.json \
  --receipt peer-receipt.json
```

A zero exit status means only that the returned bytes are a schema-valid
candidate and match the invitation. The output intentionally says
`independent_evidence_established: false` and `release_evidence: false`.
Maintainers must separately review the participant's external origin,
non-contributor status, independently controlled implementation artifact, and
release-artifact provenance before binding the receipt as detached evidence.
The validator never generates, signs, or promotes a receipt.

## Frozen boundary represented by the cases

- `offer_valid` checks exact media type, exact-byte digest, A202 content hash,
  key status, and ES256 signature.
- `offer_tampered_content`, `offer_missing_key`, and
  `offer_wrong_media_type` distinguish `failed` from `not_checkable`.
- `action_initial_head` proves the signed A202 logical Head and the unsigned
  WorldStream Room witness remain separate.
- the three retry cases prove byte reuse only for a Room-only Head change and
  rebuild-and-resign for either A202 logical Head change.

The pinned profile is the operated single-session formation profile in ADR
0015. This kit makes no general A202 conformance claim.
