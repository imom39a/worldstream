# PROTOTYPE — Portable Activity Pack execution

This throwaway prototype asks whether WorldStream can keep one language-neutral,
five-operation Activity Pack contract while loading a local, WASI-free
WebAssembly Component under deterministic limits. It also compares the first
TypeScript and Go authoring journeys rather than assuming either is ready.

Nothing in this directory is production Pack Host code or a frozen public ABI.
The experiment must earn that decision with executable evidence.

## Question

Can a meaningful two-Role negotiation Pack be built without Rust, installed
from retained original Component bytes, invoked synchronously with no ambient
capabilities, contained by deterministic resource limits, reconstructed after
restart, and Replay-equivalent through fresh callback instances?

The prototype is complete only when it records an honest pass, fallback, or
stop verdict for every gate in the ticket.

## Run it

Prerequisites are a current Rust toolchain and Node.js with npm. The script
pins the TypeScript and Jco versions used by the experiment, type-checks the
guest, builds it with every ambient capability disabled, and runs the Rust
host proofs:

```console
./run.sh
```

The run writes machine-readable evidence to `artifacts/evidence.json`. Build
products, the content-addressed scratch store, and Wasmtime's disposable cache
are deliberately ignored by Git. See `RESULTS.md` for the decision and the
limits of what this prototype proves.
