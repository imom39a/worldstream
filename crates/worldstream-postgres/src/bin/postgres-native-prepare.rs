use std::env;

use worldstream_postgres::native_restore::{
    NativePostgresEndpointV1, rebuild_native_snapshot_cache,
};

fn main() {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        eprintln!("native PostgreSQL preparation: invalid argument count");
        std::process::exit(12);
    }
    let Ok(endpoint) =
        NativePostgresEndpointV1::new(&args[1], args[2].parse().unwrap_or(0), &args[3], &args[4])
    else {
        eprintln!("native PostgreSQL preparation: invalid endpoint");
        std::process::exit(12);
    };
    if rebuild_native_snapshot_cache(&endpoint, std::path::Path::new(&args[5])).is_err() {
        eprintln!("native PostgreSQL preparation: snapshot rebuild failed");
        std::process::exit(13);
    }
}
