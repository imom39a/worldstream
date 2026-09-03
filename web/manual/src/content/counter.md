# Counter example

Counter is the smallest reference Activity Pack. It exists to explain the
kernel contract. It is not a product workflow and it is not the recommended
first user demonstration.

Its state contains one integer. A legal action increments that integer. The
example makes five kernel properties easy to inspect:

1. the Pack validates an offered action;
2. the Runtime commits the accepted change in one order;
3. deterministic reduction produces the same state during replay;
4. each participant receives only its authorized Projection; and
5. retrying one operation ID does not create a second commit.

Use Negotiate or Agent Heist for the complete local product path. The
[Getting started guide](https://github.com/imom39a/worldstream/blob/main/docs/getting-started.md)
shows both through the CLI, independent browser Activity Clients, and Python
SDK. Use Counter only when developing or testing the low-level Pack contract.

The Pack source is in
[`examples/counter`](https://github.com/imom39a/worldstream/tree/main/examples/counter).
Inspect the current example README and command help before running it because
Counter fixtures are maintainer tools, not a supported operator interface.

An Activity Client for Counter, if one is built, remains a separate client. It
must start from an authorized Projection Reset and apply ordered Observation
Frames. It must not read canonical storage, infer hidden state, or move
Pack-specific rendering into `worldstreamctl` or the Controller.
