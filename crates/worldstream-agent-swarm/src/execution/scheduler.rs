//! Deterministic global per-provider scheduling.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::provider::ProviderKind;

const MAX_PRIORITY: u8 = 16;

/// Why an Invocation is eligible. Reviews sort ahead of ordinary work only
/// after a Swarm has received its fair provider share.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationKind {
    ProgressReview,
    Work,
}

/// Durable admission identity. It contains no prompt or provider credential;
/// those are refreshed only after admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationTicket {
    pub invocation_id: String,
    pub swarm_id: String,
    pub member_id: String,
    pub provider: ProviderKind,
    pub configuration_revision: u64,
    pub kind: InvocationKind,
    pub due_sequence: u64,
}

/// One provider's configured global limit and observed active count.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapacity {
    pub limit: u16,
    pub active: u16,
}

impl ProviderCapacity {
    #[must_use]
    pub fn available(self) -> usize {
        usize::from(self.limit.saturating_sub(self.active))
    }
}

/// One deterministic scheduler result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleDecision {
    pub ticket: InvocationTicket,
    pub provider_slot: u16,
}

/// Persisted fair cursors. TUI focus is deliberately absent from scheduler
/// input and therefore cannot affect these decisions.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerState {
    cursors: BTreeMap<ProviderKind, u64>,
}

impl SchedulerState {
    /// Selects bounded work under provider caps and one-active-Invocation per
    /// roster member. Weighted round robin gives priority `n` exactly `n`
    /// positions in the provider ring, with stable identifier tie breaks.
    #[must_use]
    pub fn schedule(
        &mut self,
        queued: &[InvocationTicket],
        active: &[InvocationTicket],
        capacities: &BTreeMap<ProviderKind, ProviderCapacity>,
        priorities: &BTreeMap<String, u8>,
        blocked_swarms: &BTreeSet<String>,
    ) -> Vec<ScheduleDecision> {
        let mut remaining = queued.to_vec();
        let mut active_members = active
            .iter()
            .map(|ticket| (ticket.swarm_id.clone(), ticket.member_id.clone()))
            .collect::<BTreeSet<_>>();
        let mut decisions = Vec::new();
        for (provider, capacity) in capacities {
            let already_active = active
                .iter()
                .filter(|ticket| ticket.provider == *provider)
                .count();
            let effective_active = usize::from(capacity.active).max(already_active);
            let available = usize::from(capacity.limit).saturating_sub(effective_active);
            for ordinal in 0..available {
                let Some(index) = self.select_index(
                    *provider,
                    &remaining,
                    &active_members,
                    priorities,
                    blocked_swarms,
                ) else {
                    break;
                };
                let ticket = remaining.remove(index);
                active_members.insert((ticket.swarm_id.clone(), ticket.member_id.clone()));
                let provider_slot =
                    u16::try_from(effective_active + ordinal + 1).unwrap_or(u16::MAX);
                decisions.push(ScheduleDecision {
                    ticket,
                    provider_slot,
                });
            }
        }
        decisions
    }

    fn select_index(
        &mut self,
        provider: ProviderKind,
        remaining: &[InvocationTicket],
        active_members: &BTreeSet<(String, String)>,
        priorities: &BTreeMap<String, u8>,
        blocked_swarms: &BTreeSet<String>,
    ) -> Option<usize> {
        let eligible_swarms = remaining
            .iter()
            .filter(|ticket| {
                ticket.provider == provider
                    && !blocked_swarms.contains(&ticket.swarm_id)
                    && !active_members
                        .contains(&(ticket.swarm_id.clone(), ticket.member_id.clone()))
            })
            .map(|ticket| ticket.swarm_id.clone())
            .collect::<BTreeSet<_>>();
        if eligible_swarms.is_empty() {
            return None;
        }
        let mut ring = Vec::new();
        for swarm_id in &eligible_swarms {
            let weight = priorities
                .get(swarm_id)
                .copied()
                .unwrap_or(1)
                .clamp(1, MAX_PRIORITY);
            for _ in 0..weight {
                ring.push(swarm_id.clone());
            }
        }
        let cursor = self.cursors.entry(provider).or_default();
        let start = usize::try_from(*cursor).unwrap_or(0) % ring.len();
        let mut selected = None;
        for offset in 0..ring.len() {
            let ring_index = (start + offset) % ring.len();
            let swarm = &ring[ring_index];
            if remaining.iter().any(|ticket| {
                ticket.provider == provider
                    && &ticket.swarm_id == swarm
                    && !active_members
                        .contains(&(ticket.swarm_id.clone(), ticket.member_id.clone()))
            }) {
                *cursor = u64::try_from((ring_index + 1) % ring.len()).unwrap_or(0);
                selected = Some(swarm.as_str());
                break;
            }
        }
        let selected = selected?;
        remaining
            .iter()
            .enumerate()
            .filter(|(_, ticket)| {
                ticket.provider == provider
                    && ticket.swarm_id == selected
                    && !active_members
                        .contains(&(ticket.swarm_id.clone(), ticket.member_id.clone()))
            })
            .min_by(|(_, left), (_, right)| ticket_order(left).cmp(&ticket_order(right)))
            .map(|(index, _)| index)
    }
}

fn ticket_order(ticket: &InvocationTicket) -> (u8, u64, &str, &str) {
    let kind = match ticket.kind {
        InvocationKind::ProgressReview => 0,
        InvocationKind::Work => 1,
    };
    (
        kind,
        ticket.due_sequence,
        &ticket.member_id,
        &ticket.invocation_id,
    )
}
