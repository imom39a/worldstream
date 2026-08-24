# WorldStream Studio

WorldStream Studio is the local host-operator portal. It is a companion control
plane, not the Participant Console, and `worldstreamd` remains the only
authoritative Room runtime.

## Run locally

Start `worldstreamd` with the development configuration in one terminal:

```bash
cargo run -p worldstream-server --bin worldstreamd -- --config config/development.toml
```

Then start the Studio Supervisor and separately served portal in another:

```bash
pnpm studio:dev
```

Open <http://127.0.0.1:5174>. The portal asks the Supervisor at
`127.0.0.1:9420` for a live, typed daemon snapshot. The Supervisor probes the
daemon at `127.0.0.1:9410` and exposes only
`GET /api/v1/daemon/status`.

To run only the Supervisor, use:

```bash
pnpm studio:supervisor
```

The Supervisor accepts `--bind`, `--daemon`, and `--probe-timeout-ms` options.
It does not expose arbitrary command execution or direct Room mutation.
