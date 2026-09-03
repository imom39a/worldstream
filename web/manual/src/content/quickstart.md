# Local quickstart

WorldStream is CLI-first. The retired Studio web application is not required.
The `worldstreamctl` CLI uses a small headless Controller to manage the local
Runtime and to prepare scoped participant connections.

For the complete first-time procedure, follow the repository
[Getting started guide](https://github.com/imom39a/worldstream/blob/main/docs/getting-started.md).
It gives one ordered path with exact commands for:

1. installing the pinned Rust, Python, Node.js, and pnpm versions;
2. building `worldstreamctl`, `worldstreamd`, the Controller, assignment MCP,
   Python SDK, and browser Activity Clients;
3. initializing owner-protected local state;
4. reviewing and importing the checked-in client declarations;
5. proving, approving, installing, and selecting Negotiate while the Runtime
   is stopped;
6. starting the Controller and Runtime and checking status and logs;
7. running a Negotiate Room;
8. running separate Agent Heist Rooms through the browser client and Python
   SDK; and
9. inspecting and stopping the installation.

The guide uses these local endpoints:

| Service | Address | Purpose |
| --- | --- | --- |
| Runtime | `127.0.0.1:9410` | authoritative Room API and streams |
| Controller | `127.0.0.1:9420` | bounded local process and connection service |
| Activity Client Host | `http://127.0.0.1:5173` | independent browser clients |

The Controller binary is still named `worldstream-studio-supervisor`, and its
default state directory is still `.worldstream/studio`. These are compatibility
names. They do not install or start a Studio web application.

Do not treat a successful browser page load as proof that a Room is ready.
Use `worldstreamctl server status`, the returned operation result, and the
authorized client state. This quickstart is an MVP development flow, not proof
of production deployment or external release qualification.

## Useful checks

Run commands from the repository root:

```sh
target/debug/worldstreamctl --help
target/debug/worldstreamctl server --help
target/debug/worldstreamctl room --help
target/debug/worldstreamctl client --help
pnpm activity-clients:verify
```

Activity Clients are independent applications. A browser client, terminal
client, Python program, or agent can connect to the same Room contract. The
operator CLI does not render Pack-specific UI.
