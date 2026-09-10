//! Reviewed hosted artifacts shared by Controller startup and its conformance tests.

use anyhow::Result;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{HouseAgentRevision, ListingRevision};

/// Loads immutable reviewed artifacts without granting any Host approval.
///
/// # Errors
/// Fails if an embedded artifact is not a valid canonical contract document.
pub fn reviewed_hosted_artifacts() -> Result<(Vec<ListingRevision>, Vec<HouseAgentRevision>)> {
    const LISTINGS: &[&[u8]] = &[
        include_bytes!("../../../config/hosted/listings/midnight-archive-0.1.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.2.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.3.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.4.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.5.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.6.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.7.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.8.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.9.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.10.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.11.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.12.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.13.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.14.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.15.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.16.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.17.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.18.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.19.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.20.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.21.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.22.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.23.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.24.0.json"),
    ];
    const HOUSE_AGENTS: &[&[u8]] = &[
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-1.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-2.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-3.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-4.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-5.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-6.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-7.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-8.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-9.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-10.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-11.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-12.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-13.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-14.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-15.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-16.json"),
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-17.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-1.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-2.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-3.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-4.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-5.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-6.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-7.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-8.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-9.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-10.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-11.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-12.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-13.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-14.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-15.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-16.json"),
    ];
    let listings = LISTINGS
        .iter()
        .map(|source| {
            let bytes = CanonicalJsonV1::parse(source)?.to_bytes()?;
            ListingRevision::from_canonical_bytes(&bytes)
                .map_err(|_| anyhow::anyhow!("reviewed hosted Listing is invalid"))
        })
        .collect::<Result<Vec<_>>>()?;
    let house_agents = HOUSE_AGENTS
        .iter()
        .map(|source| {
            let bytes = CanonicalJsonV1::parse(source)?.to_bytes()?;
            HouseAgentRevision::from_canonical_bytes(&bytes)
                .map_err(|_| anyhow::anyhow!("reviewed House Agent is invalid"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((listings, house_agents))
}

/// Reads a bounded, explicitly selected local qualification fixture. This does
/// not approve its Pack, Profiles, Runner Templates, credentials, or client.
///
/// # Errors
/// Rejects symlinks, oversized files, public Listings, and invalid contracts.
pub fn read_local_hosted_fixture(
    directory: &std::path::Path,
) -> Result<(Vec<ListingRevision>, Vec<HouseAgentRevision>)> {
    fn read(directory: &std::path::Path, name: &str) -> Result<Vec<u8>> {
        let path = directory.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() <= 262_144,
            "invalid fixture file"
        );
        Ok(CanonicalJsonV1::parse(&std::fs::read(path)?)?.to_bytes()?)
    }
    let listing_bytes = read(directory, "listing.json")?;
    let value: serde_json::Value = serde_json::from_slice(&listing_bytes)?;
    anyhow::ensure!(
        value["catalog"]["visibility"] == "private" || value["catalog"]["visibility"] == "unlisted",
        "fixture cannot be public"
    );
    let listing = ListingRevision::from_canonical_bytes(&listing_bytes)?;
    let mut agents = Vec::new();
    for name in ["house-agent-1.json", "house-agent-2.json"] {
        if directory.join(name).try_exists()? {
            agents.push(HouseAgentRevision::from_canonical_bytes(&read(
                directory, name,
            )?)?);
        }
    }
    Ok((vec![listing], agents))
}

#[cfg(test)]
mod tests {
    use super::reviewed_hosted_artifacts;
    use anyhow::Context as _;
    use std::collections::BTreeSet;

    #[test]
    fn reviewed_public_listings_resolve_exact_installed_pack_rules() -> anyhow::Result<()> {
        // The reviewed hosted catalog retains every Agent Heist Pack revision,
        // including revisions intentionally omitted from the base daemon image.
        let registry = worldstream_core::builtin_agent_heist_registry()?;
        let (listings, _) = reviewed_hosted_artifacts()?;
        for listing in listings {
            // Internal installed Component candidates are admitted separately
            // by their exact bundle and running Host inventory.
            if !listing.allows_result_publication() {
                continue;
            }
            let document: serde_json::Value = serde_json::from_slice(listing.canonical_bytes())?;
            let digest = document["pack"]["digest"]
                .as_str()
                .context("Pack digest missing")?
                .parse()?;
            let pack = registry.load_retained(&digest)?;
            assert_eq!(
                document["pack"]["version"],
                pack.descriptor().explanatory_version
            );
            assert_eq!(document["pack"]["id"], pack.descriptor().pack_id);
        }
        Ok(())
    }

    #[test]
    fn private_candidates_do_not_require_publication_replay() -> anyhow::Result<()> {
        let (listings, _) = reviewed_hosted_artifacts()?;
        let private: Vec<_> = listings
            .iter()
            .filter(|listing| !listing.allows_result_publication())
            .collect();
        assert_eq!(private.len(), 1);
        assert!(!private[0].allows_anonymous_viewing());
        Ok(())
    }

    #[test]
    fn controller_catalog_covers_every_gateway_listing_and_its_house_revisions()
    -> anyhow::Result<()> {
        let (listings, house_agents) = reviewed_hosted_artifacts()?;
        let fly = include_str!("../../../packaging/hosted/fly.toml");
        let declaration = fly
            .lines()
            .find(|line| line.trim().starts_with("WORLDSTREAM_LISTING_ALLOWLIST = "))
            .context("Fly allowlist declaration missing")?;
        let value: String = serde_json::from_str(
            declaration
                .split_once('=')
                .context("allowlist value missing")?
                .1
                .trim(),
        )?;
        let allowed: BTreeSet<_> = value.split(',').collect();
        let embedded: BTreeSet<_> = listings.iter().map(|listing| listing.digest()).collect();
        assert!(
            allowed.is_subset(&embedded),
            "every Gateway-admitted Listing must resolve in the Controller"
        );
        let house: BTreeSet<_> = house_agents.iter().map(|agent| agent.digest()).collect();
        for listing in listings {
            let document: serde_json::Value = serde_json::from_slice(listing.canonical_bytes())?;
            for seat in document["seats"]
                .as_array()
                .context("Listing seats missing")?
            {
                for digest in seat["allowed_house_agent_revisions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        house.contains(digest.as_str().context("House digest must be a string")?),
                        "every allowed House revision must resolve in the Controller"
                    );
                }
            }
        }
        Ok(())
    }
}
