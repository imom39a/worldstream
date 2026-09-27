//! Historical hosted catalog fixtures used only by conformance tests.

use anyhow::Result;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{HouseAgentRevision, ListingRevision};

/// Loads immutable reviewed artifacts without granting any Host approval.
///
/// # Errors
/// Fails if an embedded artifact is not a valid canonical contract document.
pub fn reviewed_hosted_artifacts() -> Result<(Vec<ListingRevision>, Vec<HouseAgentRevision>)> {
    const LISTINGS: &[&[u8]] = &[
        include_bytes!("../../../tests/fixtures/hosted/listings/midnight-archive-0.1.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/midnight-archive-0.2.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/midnight-archive-0.3.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/midnight-archive-0.4.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/midnight-archive-0.5.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.2.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.3.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.4.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.5.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.6.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.7.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.8.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.9.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.10.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.11.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.12.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.13.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.14.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.15.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.16.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.17.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.18.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.19.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.20.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.21.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.22.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.23.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.24.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.25.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.26.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.27.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.28.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.29.0.json"),
        include_bytes!("../../../tests/fixtures/hosted/listings/agent-heist-0.30.0.json"),
    ];
    const HOUSE_AGENTS: &[&[u8]] = &[
        include_bytes!("../../../tests/fixtures/hosted/house-agents/mira-1.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/mira-2.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/jonah-1.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/jonah-2.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-1.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-2.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-3.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-4.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-5.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-6.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-7.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-8.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-9.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-10.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-11.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-12.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-13.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-14.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-15.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-16.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-17.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/cooperative-planner-18.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-1.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-2.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-3.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-4.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-5.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-6.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-7.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-8.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-9.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-10.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-11.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-12.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-13.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-14.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-15.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-16.json"),
        include_bytes!("../../../tests/fixtures/hosted/house-agents/skeptical-auditor-17.json"),
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
