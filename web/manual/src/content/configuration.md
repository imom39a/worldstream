# Configuration

Configuration is startup-fixed and fail-closed. Precedence is compiled defaults,
one versioned TOML file, environment, then CLI overrides.

## Local SQLite profile

`config/development.toml` is the source-controlled local template:

```toml
config_version = 1

[server]
bind = "127.0.0.1:9410"

[storage]
profile = "sqlite-bundled"
data_dir = ".worldstream/data"
deployment_lineage = "development/local"
storage_epoch = 1

[authority.bootstrap]
secret_file = ".worldstream/authority.secret"
```

The data directory, authority file, Supervisor state, database, and generated
backup artifacts must remain outside source control.

## Selection and overrides

- `WORLDSTREAM_CONFIG` selects a configuration file when `--config` is absent.
- Nested environment keys use the `WORLDSTREAM__` prefix.
- `worldstreamd` and `worldstreamctl` support non-secret `--bind`,
  `--storage-profile`, and `--data-dir` overrides.
- Secret values belong in owner-only files or inherited handles, never CLI
  arguments or ordinary environment dumps.

Unknown keys, wrong types, inactive-profile options, partial deployment
metadata, unsafe endpoints, plaintext credential configuration, broad file
permissions, symlinks, and filesystem substitution fail closed.

## Safe inspection

```sh
target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamctl --config config/development.toml config effective
target/debug/worldstreamctl --config config/development.toml doctor
```

Effective output redacts secret path/value material. Do not use application
debug output as a secret-discovery mechanism.

## Storage profiles

| Profile | Intended use | Notes |
| --- | --- | --- |
| `sqlite-bundled` | default local/single-host runtime | exact bundled engine; owner-only local filesystem |
| `postgres-primary` | PostgreSQL 17 primary | direct/provider profile verified at startup |
| `ephemeral` | explicitly non-durable scenarios | not backup-capable and not release durability |

The profile is selected at startup. WorldStream does not dual-write or switch a
live deployment between SQLite and PostgreSQL.

## Telemetry

Structured logging is the default. Optional telemetry endpoints are validated,
bounded, and redacted. Exporter failure does not change authoritative behavior
or readiness; queue saturation is counted rather than blocking Room commits.

Source: [configuration implementation](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-runtime/src/config.rs),
[fail-closed startup](https://github.com/imom39a/worldstream/blob/main/docs/architecture.md#configuration-and-fail-closed-startup),
and [storage-profile ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0004-supported-storage-profiles-and-offline-portability.md).
