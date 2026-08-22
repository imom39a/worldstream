//! The daemon's complete embedded Activity Pack registry.
//!
//! Each source registry is independently validated before composition. The
//! composition step is deliberately narrow: it retains the exact Counter
//! revisions used by local conformance stories and the exact Agent Heist
//! revision used by the live story, while preserving digest-only selection.

use crate::{PackRegistryErrorV1, PackRegistryV1, agent_heist_registry, counter_registry};

/// Builds the validated registry used by the `WorldStream` daemon.
///
/// Counter v1 remains retained-only, Counter v2 remains selectable for the
/// existing Counter story, and Agent Heist remains selectable at its exact
/// reviewed revision. No name-based fallback or compatibility-manifest value
/// is introduced here.
///
/// # Errors
///
/// Returns the first closed registry-validation error from the embedded
/// Counter or Agent Heist registry, or a composition collision.
pub fn builtin_worldstream_registry() -> Result<PackRegistryV1, PackRegistryErrorV1> {
    PackRegistryV1::combine([
        counter_registry::builtin_counter_registry()?,
        agent_heist_registry::builtin_agent_heist_registry()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::builtin_worldstream_registry;
    use crate::{
        agent_heist_digest, agent_heist_retained_digest, counter_v1_digest, counter_v2_digest,
    };

    #[test]
    fn daemon_registry_retains_counter_and_exact_heist_revisions() {
        let registry = builtin_worldstream_registry()
            .unwrap_or_else(|error| unreachable!("WorldStream registry: {error}"));
        assert_eq!(registry.len(), 4);
        assert!(registry.load_retained(&counter_v1_digest()).is_ok());
        assert!(registry.select_for_new_room(&counter_v2_digest()).is_ok());
        assert!(registry.select_for_new_room(&agent_heist_digest()).is_ok());
        assert!(
            registry
                .load_retained(&agent_heist_retained_digest())
                .is_ok()
        );
        assert!(
            registry
                .select_for_new_room(&agent_heist_retained_digest())
                .is_err()
        );
    }
}
