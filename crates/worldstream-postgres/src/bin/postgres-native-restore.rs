use std::env;

use worldstream_postgres::native_restore::{
    NativePostgresEndpointV1, NativePostgresRestoreConfig, run_native_postgres_restore,
};

const EXIT_CONFIGURATION: i32 = 12;
const EXIT_INCOMPLETE: i32 = 13;
const EXIT_PROVIDER: i32 = 14;

fn main() {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 14 {
        eprintln!("native PostgreSQL restore: invalid argument count");
        std::process::exit(EXIT_CONFIGURATION);
    }
    let Ok(source) =
        NativePostgresEndpointV1::new(&args[1], args[2].parse().unwrap_or(0), &args[3], &args[4])
    else {
        eprintln!("native PostgreSQL restore: invalid source endpoint");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(target) =
        NativePostgresEndpointV1::new(&args[5], args[6].parse().unwrap_or(0), &args[7], &args[8])
    else {
        eprintln!("native PostgreSQL restore: invalid target endpoint");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(config) = NativePostgresRestoreConfig::new(
        source, target, &args[9], &args[10], &args[11], &args[12], &args[13],
    ) else {
        eprintln!("native PostgreSQL restore: invalid configuration");
        std::process::exit(EXIT_CONFIGURATION);
    };
    let Ok(outcome) = run_native_postgres_restore(&config) else {
        eprintln!("native PostgreSQL restore: provider operation failed");
        std::process::exit(EXIT_PROVIDER);
    };
    println!(
        "{}",
        serde_json::to_string(&outcome.report)
            .unwrap_or_else(|_| "{\"status\":\"incomplete\",\"secrets_emitted\":false}".to_owned())
    );
    if outcome.report.status == "ready" {
        return;
    }
    std::process::exit(EXIT_INCOMPLETE);
}
