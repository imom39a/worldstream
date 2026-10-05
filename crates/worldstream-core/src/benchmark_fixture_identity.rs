//! Recorded dependency identity for the two synthetic conformance benchmarks.
//!
//! This input is the exact workspace Cargo.lock used to author their fixed
//! golden vectors. It is not the current workspace dependency graph. Its
//! SHA-256 is e83c8fbd042bfb860355a5fb353feb0fd2faed6ff1ee21e607d65164faeecddd.
//!
//! The scoped import after each executor's evidence boundary preserves its
//! original source-prefix artifact digest. Only the original root-lock literal
//! is accepted; other include_bytes calls cannot silently use this fixture.
macro_rules! recorded_dependency_lock {
    ("../../../Cargo.lock") => {
        ::std::include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/benchmark-dependency.lock"
        ))
    };
}
pub(super) use recorded_dependency_lock as include_bytes;
