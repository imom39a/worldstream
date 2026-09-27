# WorldStream Negotiate oracle

This crate is the independent, conformance-only executable specification for
the first `worldstream.negotiate` profile. It has no dependency on
`worldstream-core`, performs no runtime I/O, and must never be installed as the
production Activity Pack executor.

`corpus_bytes()` returns the canonical machine-readable comparison corpus for
IMO-119; `fixtures/corpus-v1.json` is its checked-in, cross-language mirror. The
corpus contains the exact four Roles, eight phases, eleven Actions,
four Attention reasons, timer rules, six-persona privacy matrix, golden
restart/reconnect transcript, exact byte-bearing A202-shaped objects,
independent Room/transaction/session Heads, both Outcomes, evidence
cross-index, and all required negative-case identities.

The fixture signature proofs are deterministic conformance witnesses. They
freeze exact-byte, signer, and purpose binding without pretending to be real
A202 credentials. A production Negotiate Pack must additionally run the pinned
A202 canonicalizer, schemas, SHA-256 rules, signature algorithms, mandates, and
resolver checks from revision
`fa85aa8b49bfe7b3f7ded487c98500a600e92e41`.
