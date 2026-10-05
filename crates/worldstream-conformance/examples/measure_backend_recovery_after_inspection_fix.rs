//! Separate entry so an updated release does not replace running matrix binaries.
#[path = "measure_backend_stream_scaling.rs"]
mod backend;

fn main() {
    backend::main();
}
