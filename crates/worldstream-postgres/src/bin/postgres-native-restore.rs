use std::{env, io::Write as _};

use worldstream_postgres::native_restore::{
    NATIVE_POSTGRES_ADMISSION_ARG_V1, NATIVE_POSTGRES_COMMIT_ARG_V1, NATIVE_POSTGRES_REPAIR_ARG_V1,
    NATIVE_POSTGRES_WORKER_ARG_V1, NativePostgresArtifactDirectoryIdentityV1,
    NativePostgresEndpointV1, NativePostgresRestoreConfig, NativePostgresTlsModeV1,
    execute_native_postgres_restore_admission_worker,
    execute_native_postgres_restore_commit_worker, execute_native_postgres_restore_repair_worker,
    execute_native_postgres_restore_worker, run_native_postgres_restore,
};

const EXIT_CONFIGURATION: i32 = 12;
const EXIT_INCOMPLETE: i32 = 13;
const EXIT_PROVIDER: i32 = 14;

#[allow(clippy::too_many_lines)]
fn main() {
    let args = env::args().collect::<Vec<_>>();
    if args.len() == 2 && args[1] == NATIVE_POSTGRES_WORKER_ARG_V1 {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        match execute_native_postgres_restore_worker(stdin.lock(), stdout.lock(), stderr.lock()) {
            Ok(true) => return,
            Ok(false) | Err(_) => std::process::exit(EXIT_PROVIDER),
        }
    }
    if args.len() == 2 && args[1] == NATIVE_POSTGRES_REPAIR_ARG_V1 {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        match execute_native_postgres_restore_repair_worker(stdin.lock(), stdout.lock()) {
            Ok(()) => return,
            Err(_) => std::process::exit(EXIT_PROVIDER),
        }
    }
    if args.len() == 2 && args[1] == NATIVE_POSTGRES_ADMISSION_ARG_V1 {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        match execute_native_postgres_restore_admission_worker(stdin.lock(), stdout.lock()) {
            Ok(()) => return,
            Err(_) => std::process::exit(EXIT_PROVIDER),
        }
    }
    if args.len() == 2 && args[1] == NATIVE_POSTGRES_COMMIT_ARG_V1 {
        match execute_native_postgres_restore_commit_worker() {
            Ok(()) => return,
            Err(_) => std::process::exit(EXIT_PROVIDER),
        }
    }
    if args.len() != 19 {
        eprintln!("native PostgreSQL restore: invalid argument count");
        std::process::exit(EXIT_CONFIGURATION);
    }
    let Ok(source_tls_mode) = args[5].parse::<NativePostgresTlsModeV1>() else {
        eprintln!("native PostgreSQL restore: invalid source TLS mode");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(source) = NativePostgresEndpointV1::new(
        &args[1],
        args[2].parse().unwrap_or(0),
        &args[3],
        &args[4],
        source_tls_mode,
    ) else {
        eprintln!("native PostgreSQL restore: invalid source endpoint");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(target_tls_mode) = args[10].parse::<NativePostgresTlsModeV1>() else {
        eprintln!("native PostgreSQL restore: invalid target TLS mode");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(target) = NativePostgresEndpointV1::new(
        &args[6],
        args[7].parse().unwrap_or(0),
        &args[8],
        &args[9],
        target_tls_mode,
    ) else {
        eprintln!("native PostgreSQL restore: invalid target endpoint");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(artifact_directory_identity) =
        NativePostgresArtifactDirectoryIdentityV1::new(&args[16], &args[17])
    else {
        eprintln!("native PostgreSQL restore: invalid artifact directory identity");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(config) = NativePostgresRestoreConfig::new(
        source,
        target,
        &args[11],
        &args[12],
        &args[13],
        &args[14],
        &args[15],
        artifact_directory_identity,
    ) else {
        eprintln!("native PostgreSQL restore: invalid configuration");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let outcome = match run_native_postgres_restore(
        &config,
        std::path::Path::new(&args[18]),
        std::time::Duration::from_mins(20),
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("native PostgreSQL restore: provider operation failed: {error}");
            std::process::exit(EXIT_PROVIDER);
        }
    };
    let report = outcome.report();
    let serialized = serde_json::to_string(report)
        .unwrap_or_else(|_| "{\"status\":\"incomplete\",\"secrets_emitted\":false}".to_owned());
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    if writeln!(output, "{serialized}")
        .and_then(|()| output.flush())
        .is_err()
    {
        std::process::exit(EXIT_PROVIDER);
    }
    if report.status == "ready" {
        return;
    }
    std::process::exit(EXIT_INCOMPLETE);
}
