use std::env;

use worldstream_postgres::native_restore::{
    NativePostgresEndpointV1, NativePostgresTlsModeV1, rebuild_native_snapshot_cache,
};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 7 {
        eprintln!("native PostgreSQL preparation: invalid argument count");
        std::process::exit(12);
    }
    let Ok(tls_mode) = args[5].parse::<NativePostgresTlsModeV1>() else {
        eprintln!("native PostgreSQL preparation: invalid TLS mode");
        std::process::exit(12);
    };
    let Ok(endpoint) = NativePostgresEndpointV1::new(
        &args[1],
        args[2].parse().unwrap_or(0),
        &args[3],
        &args[4],
        tls_mode,
    ) else {
        eprintln!("native PostgreSQL preparation: invalid endpoint");
        std::process::exit(12);
    };
    if let Err(error) = rebuild_native_snapshot_cache(&endpoint, std::path::Path::new(&args[6])) {
        eprintln!("native PostgreSQL preparation: snapshot rebuild failed: {error}");
        std::process::exit(13);
    }
}
