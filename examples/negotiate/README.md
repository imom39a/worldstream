# Negotiate

An Activity Pack example for a bounded multi-party negotiation: two commercial
agents, a human approver, and an external signer. It demonstrates that approval,
signature evidence, and deadlines can be application rules while WorldStream
owns ordering, authorization, persistence, and Replay.

- [TypeScript Pack and build instructions](../packs/negotiate/README.md)
- [Rule design](design.md)
- [Independent oracle and test corpus](oracle/README.md)
- [A202 adapter](a202-adapter/README.md)
- [Offline evidence verifier](evidence-verifier/README.md)
- [Browser client](../clients/negotiate-web/README.md)

The protocol profile and retained fixtures are historical research inputs.
They do not establish current interoperability with external services or a
commercially supported negotiation product.

```sh
cargo test --locked -p worldstream-negotiate-oracle
cargo test --locked -p worldstream-a202-adapter
cargo test --locked -p worldstream-negotiate-evidence
```
