# WorldStream Negotiate offline verifier

`worldstream-negotiate-evidence` verifies an exported
`worldstream/negotiate-proof-package/v1` without contacting a WorldStream
operator. Its production dependency graph does not contain the Room Kernel,
the A202 adapter, or the native Negotiate oracle.

It reports every check as exactly one of:

- `verified`: the check ran and passed;
- `failed`: the check ran and did not pass; or
- `not_checkable`: required material or trust was not supplied.

There is intentionally no overall boolean in the JSON report.

Run it with:

```text
worldstream-negotiate-verify PROOF.json \
  --pack worldstream.negotiate.wspack \
  --trust verifier-trust.json
```

The trust file uses this shape:

```json
{
  "a202_keys": [
    {
      "key_id": "key_example",
      "subject_id": "agt_example",
      "public_key_sec1_base64url": "..."
    }
  ],
  "resolver_sources": [
    {
      "source_id": "host-source-example",
      "key_id": "key_resolver_example",
      "public_key_sec1_base64url": "..."
    }
  ]
}
```

The verifier checks the party/protocol and venue/runtime sections and their
cross-index. Proof-package v1 carries exact venue records and a Replay report,
but not the complete canonical Genesis/Transition inputs needed to execute the
Activity Pack independently. It therefore reports full Room Replay as
`not_checkable`; it never upgrades an exported `declared_result` to verified.
