#![cfg(feature = "managed-local-runtime")]

use std::{collections::BTreeMap, error::Error};

use worldstream_agent_swarm::execution::{
    DesiredExecution, ExecutionCommand, ExecutionError, ExecutionPhase, ExecutionSnapshot,
    ExecutionSupervisor, InvocationKind, InvocationResolution, InvocationTicket,
    MemoryExecutionJournal, ProviderKind, RunBudget, SwarmExecutionSnapshot,
};

#[test]
fn roster_registration_admits_work_without_manual_provider_policy() -> Result<(), Box<dyn Error>> {
    let mut supervisor = ExecutionSupervisor::open(MemoryExecutionJournal::new(), 0)?;
    register_with_roster(
        &mut supervisor,
        "swarm-roster",
        BTreeMap::from([(ProviderKind::Controlled, 1)]),
        0,
    )?;
    supervisor.command(
        ExecutionCommand::Resume {
            swarm_id: "swarm-roster".to_owned(),
        },
        0,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "roster-work",
            "swarm-roster",
            "member-roster",
            InvocationKind::Work,
            1,
        ),
        1,
    )?;

    assert_eq!(
        supervisor.snapshot(1).provider_caps,
        BTreeMap::from([(ProviderKind::Controlled, 1)])
    );
    let admitted = supervisor.schedule(2)?;
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].ticket.invocation_id, "roster-work");
    Ok(())
}

#[test]
fn roster_caps_use_global_max_and_never_replace_manual_override() -> Result<(), Box<dyn Error>> {
    let journal = MemoryExecutionJournal::new();
    let mut supervisor = ExecutionSupervisor::open(journal.clone(), 0)?;
    register_with_roster(
        &mut supervisor,
        "swarm-small",
        BTreeMap::from([(ProviderKind::Controlled, 2)]),
        0,
    )?;
    register_with_roster(
        &mut supervisor,
        "swarm-large",
        BTreeMap::from([(ProviderKind::Codex, 1), (ProviderKind::Controlled, 3)]),
        0,
    )?;

    // Independent Swarms share the largest selected roster count. Their
    // member counts are not added into ambient subscription concurrency.
    assert_eq!(
        supervisor.snapshot(0).provider_caps,
        BTreeMap::from([(ProviderKind::Codex, 1), (ProviderKind::Controlled, 3),])
    );

    supervisor.command(
        ExecutionCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 1,
        },
        1,
    )?;
    register_with_roster(
        &mut supervisor,
        "swarm-later",
        BTreeMap::from([(ProviderKind::Controlled, 4)]),
        2,
    )?;
    assert_eq!(
        supervisor.snapshot(2).provider_caps,
        BTreeMap::from([(ProviderKind::Codex, 1), (ProviderKind::Controlled, 1),])
    );

    supervisor.close_cleanly()?;
    let recovered = ExecutionSupervisor::open(journal, 3)?;
    assert_eq!(
        recovered.snapshot(3).provider_caps,
        BTreeMap::from([(ProviderKind::Codex, 1), (ProviderKind::Controlled, 1),])
    );
    Ok(())
}

#[test]
fn roster_counts_are_bounded_and_unselected_providers_stay_closed() -> Result<(), Box<dyn Error>> {
    let mut supervisor = ExecutionSupervisor::open(MemoryExecutionJournal::new(), 0)?;
    assert!(matches!(
        supervisor.command(
            ExecutionCommand::RegisterSwarm {
                swarm_id: "invalid-zero".to_owned(),
                priority: 1,
                budget: RunBudget::default(),
                roster_provider_counts: BTreeMap::from([(ProviderKind::Codex, 0)]),
            },
            0,
        ),
        Err(ExecutionError::InvalidCommand)
    ));
    assert!(matches!(
        supervisor.command(
            ExecutionCommand::RegisterSwarm {
                swarm_id: "invalid-total".to_owned(),
                priority: 1,
                budget: RunBudget::default(),
                roster_provider_counts: BTreeMap::from([
                    (ProviderKind::Claude, 8),
                    (ProviderKind::Codex, 9),
                ]),
            },
            0,
        ),
        Err(ExecutionError::InvalidCommand)
    ));

    register_with_roster(
        &mut supervisor,
        "closed-provider",
        BTreeMap::from([(ProviderKind::Controlled, 1)]),
        1,
    )?;
    supervisor.command(
        ExecutionCommand::Resume {
            swarm_id: "closed-provider".to_owned(),
        },
        1,
    )?;
    let mut codex_ticket = ticket(
        "unselected-codex",
        "closed-provider",
        "member-codex",
        InvocationKind::Work,
        1,
    );
    codex_ticket.provider = ProviderKind::Codex;
    enqueue(&mut supervisor, codex_ticket, 2)?;
    assert!(
        !supervisor
            .snapshot(2)
            .provider_caps
            .contains_key(&ProviderKind::Codex)
    );
    assert!(supervisor.schedule(3)?.is_empty());
    Ok(())
}

#[test]
fn targeted_cancel_preserves_running_swarm_and_other_active_work() -> Result<(), Box<dyn Error>> {
    let mut supervisor = ExecutionSupervisor::open(MemoryExecutionJournal::new(), 0)?;
    register_with_roster(
        &mut supervisor,
        "swarm-cancel",
        BTreeMap::from([(ProviderKind::Controlled, 2)]),
        0,
    )?;
    supervisor.command(
        ExecutionCommand::Resume {
            swarm_id: "swarm-cancel".to_owned(),
        },
        0,
    )?;
    for (invocation_id, member_id) in [("cancel-me", "member-a"), ("keep-running", "member-b")] {
        enqueue(
            &mut supervisor,
            ticket(
                invocation_id,
                "swarm-cancel",
                member_id,
                InvocationKind::Work,
                1,
            ),
            1,
        )?;
    }
    assert_eq!(supervisor.schedule(2)?.len(), 2);

    let requested = supervisor.command(
        ExecutionCommand::CancelInvocation {
            swarm_id: "swarm-cancel".to_owned(),
            invocation_id: "cancel-me".to_owned(),
        },
        3,
    )?;
    assert_eq!(requested.stop_invocations, vec!["cancel-me"]);
    supervisor.command(
        ExecutionCommand::ResolveInvocation {
            swarm_id: "swarm-cancel".to_owned(),
            invocation_id: "cancel-me".to_owned(),
            resolution: InvocationResolution::Terminated,
        },
        4,
    )?;

    let snapshot = supervisor.snapshot(4);
    let swarm = swarm(&snapshot, "swarm-cancel")?;
    assert_eq!(swarm.desired, DesiredExecution::Running);
    assert_eq!(swarm.phase, ExecutionPhase::Running);
    assert_eq!(swarm.active.len(), 1);
    assert_eq!(swarm.active[0].ticket.invocation_id, "keep-running");

    // A lost reply may retry only this exact retained terminal cancellation.
    let duplicate = supervisor.command(
        ExecutionCommand::CancelInvocation {
            swarm_id: "swarm-cancel".to_owned(),
            invocation_id: "cancel-me".to_owned(),
        },
        5,
    )?;
    assert!(duplicate.stop_invocations.is_empty());
    Ok(())
}

#[test]
fn cancellation_crash_window_reopens_as_retained_unknown() -> Result<(), Box<dyn Error>> {
    let journal = MemoryExecutionJournal::new();
    let mut supervisor = ExecutionSupervisor::open(journal.clone(), 0)?;
    register_with_roster(
        &mut supervisor,
        "swarm-cancel-crash",
        BTreeMap::from([(ProviderKind::Controlled, 1)]),
        0,
    )?;
    supervisor.command(
        ExecutionCommand::Resume {
            swarm_id: "swarm-cancel-crash".to_owned(),
        },
        0,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "cancel-uncertain",
            "swarm-cancel-crash",
            "member-a",
            InvocationKind::Work,
            1,
        ),
        1,
    )?;
    assert_eq!(supervisor.schedule(2)?.len(), 1);
    supervisor.command(
        ExecutionCommand::CancelInvocation {
            swarm_id: "swarm-cancel-crash".to_owned(),
            invocation_id: "cancel-uncertain".to_owned(),
        },
        3,
    )?;
    drop(supervisor);

    let mut recovered = ExecutionSupervisor::open(journal, 4)?;
    let snapshot = recovered.snapshot(4);
    let recovered_swarm = swarm(&snapshot, "swarm-cancel-crash")?;
    assert_eq!(recovered_swarm.phase, ExecutionPhase::RecoveryRequired);
    assert_eq!(
        recovered_swarm.active[0].resolution,
        InvocationResolution::Unknown
    );
    let duplicate = recovered.command(
        ExecutionCommand::CancelInvocation {
            swarm_id: "swarm-cancel-crash".to_owned(),
            invocation_id: "cancel-uncertain".to_owned(),
        },
        5,
    )?;
    assert!(duplicate.stop_invocations.is_empty());

    // External reconciliation may later prove that containment is terminal.
    recovered.command(
        ExecutionCommand::ResolveInvocation {
            swarm_id: "swarm-cancel-crash".to_owned(),
            invocation_id: "cancel-uncertain".to_owned(),
            resolution: InvocationResolution::Terminated,
        },
        6,
    )?;
    assert!(
        swarm(&recovered.snapshot(6), "swarm-cancel-crash")?
            .active
            .is_empty()
    );
    Ok(())
}

#[test]
fn global_cap_and_fair_cursor_survive_durable_scheduling_rounds() -> Result<(), Box<dyn Error>> {
    let journal = MemoryExecutionJournal::new();
    let mut supervisor = ExecutionSupervisor::open(journal.clone(), 0)?;
    register_and_resume(&mut supervisor, "swarm-a", RunBudget::default(), 0)?;
    register_and_resume(&mut supervisor, "swarm-b", RunBudget::default(), 0)?;
    supervisor.command(
        ExecutionCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 1,
        },
        0,
    )?;

    enqueue(
        &mut supervisor,
        ticket(
            "a-work",
            "swarm-a",
            "member-a-work",
            InvocationKind::Work,
            1,
        ),
        1,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "a-review",
            "swarm-a",
            "member-a-review",
            InvocationKind::ProgressReview,
            2,
        ),
        1,
    )?;
    enqueue(
        &mut supervisor,
        ticket("b-work", "swarm-b", "member-b", InvocationKind::Work, 1),
        1,
    )?;

    // Equal-priority Swarm A receives the first share, and its due progress
    // review sorts ahead of ordinary work only within that share.
    let first = supervisor.schedule(2)?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].ticket.invocation_id, "a-review");
    assert_eq!(first[0].provider_slot, 1);
    assert_eq!(active_count(&supervisor.snapshot(2)), 1);

    // The provider cap is global: Swarm B cannot consume another slot while
    // Swarm A's Invocation is still active.
    assert!(supervisor.schedule(3)?.is_empty());
    resolve(&mut supervisor, "swarm-a", "a-review", 4)?;

    // The fair cursor is durable operational state. A clean coordinator
    // restart between terminal-resolution rounds cannot reset it in Swarm A's
    // favor or make Swarm B wait through another A admission.
    supervisor.close_cleanly()?;
    let mut supervisor = ExecutionSupervisor::open(journal, 5)?;
    let second = supervisor.schedule(6)?;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].ticket.invocation_id, "b-work");
    assert_eq!(active_count(&supervisor.snapshot(6)), 1);
    assert!(supervisor.schedule(7)?.is_empty());

    resolve(&mut supervisor, "swarm-b", "b-work", 8)?;
    let third = supervisor.schedule(9)?;
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].ticket.invocation_id, "a-work");
    resolve(&mut supervisor, "swarm-a", "a-work", 10)?;
    supervisor.close_cleanly()?;
    Ok(())
}

#[test]
fn invocation_budget_drains_to_pause_and_preserves_work_for_other_swarms()
-> Result<(), Box<dyn Error>> {
    let mut supervisor = ExecutionSupervisor::open(MemoryExecutionJournal::new(), 0)?;
    register_and_resume(
        &mut supervisor,
        "swarm-budgeted",
        RunBudget {
            invocation_limit: Some(1),
            active_time_limit_ms: None,
        },
        0,
    )?;
    register_and_resume(&mut supervisor, "swarm-peer", RunBudget::default(), 0)?;
    supervisor.command(
        ExecutionCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 1,
        },
        0,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "budgeted-first",
            "swarm-budgeted",
            "budgeted-member-1",
            InvocationKind::Work,
            1,
        ),
        1,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "budgeted-preserved",
            "swarm-budgeted",
            "budgeted-member-2",
            InvocationKind::Work,
            2,
        ),
        1,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "peer-work",
            "swarm-peer",
            "peer-member",
            InvocationKind::Work,
            1,
        ),
        1,
    )?;

    let admission = supervisor.schedule(2)?;
    assert_eq!(admission.len(), 1);
    assert_eq!(admission[0].ticket.invocation_id, "budgeted-first");
    let snapshot = supervisor.snapshot(2);
    let budgeted = swarm(&snapshot, "swarm-budgeted")?;
    assert_eq!(budgeted.desired, DesiredExecution::Paused);
    assert_eq!(budgeted.phase, ExecutionPhase::Pausing);
    assert_eq!(budgeted.invocations_started, 1);
    assert_eq!(budgeted.queued.len(), 1);
    assert_eq!(budgeted.queued[0].invocation_id, "budgeted-preserved");
    assert_eq!(active_count(&snapshot), 1);

    // Pause uses drain semantics: the admitted Invocation keeps the only slot
    // until it settles, and no additional budgeted work starts.
    assert!(supervisor.schedule(3)?.is_empty());
    resolve(&mut supervisor, "swarm-budgeted", "budgeted-first", 4)?;
    let snapshot = supervisor.snapshot(4);
    let budgeted = swarm(&snapshot, "swarm-budgeted")?;
    assert_eq!(budgeted.phase, ExecutionPhase::Paused);
    assert_eq!(budgeted.queued.len(), 1);

    // Exhausting one Swarm's optional budget does not stall an independent
    // Swarm waiting on the shared provider.
    let peer_admission = supervisor.schedule(5)?;
    assert_eq!(peer_admission.len(), 1);
    assert_eq!(peer_admission[0].ticket.invocation_id, "peer-work");
    assert_eq!(active_count(&supervisor.snapshot(5)), 1);
    resolve(&mut supervisor, "swarm-peer", "peer-work", 6)?;
    Ok(())
}

#[test]
fn active_time_budget_requests_pause_at_the_declared_boundary() -> Result<(), Box<dyn Error>> {
    let mut supervisor = ExecutionSupervisor::open(MemoryExecutionJournal::new(), 0)?;
    register_and_resume(
        &mut supervisor,
        "swarm-timed",
        RunBudget {
            invocation_limit: None,
            active_time_limit_ms: Some(50),
        },
        0,
    )?;
    register_and_resume(&mut supervisor, "swarm-peer", RunBudget::default(), 0)?;
    supervisor.command(
        ExecutionCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 1,
        },
        0,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "timed-active",
            "swarm-timed",
            "timed-member-1",
            InvocationKind::Work,
            1,
        ),
        1,
    )?;
    enqueue(
        &mut supervisor,
        ticket(
            "timed-preserved",
            "swarm-timed",
            "timed-member-2",
            InvocationKind::Work,
            2,
        ),
        1,
    )?;
    let admission = supervisor.schedule(10)?;
    assert_eq!(admission.len(), 1);
    assert_eq!(admission[0].ticket.invocation_id, "timed-active");
    enqueue(
        &mut supervisor,
        ticket(
            "peer-after-time-budget",
            "swarm-peer",
            "peer-member",
            InvocationKind::Work,
            1,
        ),
        11,
    )?;
    assert_eq!(
        swarm(&supervisor.snapshot(59), "swarm-timed")?.phase,
        ExecutionPhase::Running
    );

    // Scheduling at the exact cumulative active-time limit ticks the budget.
    // The active turn drains, queued work remains, and no peer can exceed the
    // global cap while that drain is in progress.
    assert!(supervisor.schedule(60)?.is_empty());
    let snapshot = supervisor.snapshot(60);
    let timed = swarm(&snapshot, "swarm-timed")?;
    assert_eq!(timed.desired, DesiredExecution::Paused);
    assert_eq!(timed.phase, ExecutionPhase::Pausing);
    assert_eq!(timed.active_time_ms, 50);
    assert_eq!(timed.queued.len(), 1);
    assert_eq!(timed.queued[0].invocation_id, "timed-preserved");

    resolve(&mut supervisor, "swarm-timed", "timed-active", 70)?;
    assert_eq!(
        swarm(&supervisor.snapshot(70), "swarm-timed")?.phase,
        ExecutionPhase::Paused
    );
    let peer_admission = supervisor.schedule(71)?;
    assert_eq!(peer_admission.len(), 1);
    assert_eq!(
        peer_admission[0].ticket.invocation_id,
        "peer-after-time-budget"
    );
    resolve(&mut supervisor, "swarm-peer", "peer-after-time-budget", 72)?;
    Ok(())
}

fn register_and_resume(
    supervisor: &mut ExecutionSupervisor<MemoryExecutionJournal>,
    swarm_id: &str,
    budget: RunBudget,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    supervisor.command(
        ExecutionCommand::RegisterSwarm {
            swarm_id: swarm_id.to_owned(),
            priority: 1,
            budget,
            roster_provider_counts: BTreeMap::new(),
        },
        now_ms,
    )?;
    supervisor.command(
        ExecutionCommand::Resume {
            swarm_id: swarm_id.to_owned(),
        },
        now_ms,
    )?;
    Ok(())
}

fn register_with_roster(
    supervisor: &mut ExecutionSupervisor<MemoryExecutionJournal>,
    swarm_id: &str,
    roster_provider_counts: BTreeMap<ProviderKind, u16>,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    supervisor.command(
        ExecutionCommand::RegisterSwarm {
            swarm_id: swarm_id.to_owned(),
            priority: 1,
            budget: RunBudget::default(),
            roster_provider_counts,
        },
        now_ms,
    )?;
    Ok(())
}

fn enqueue(
    supervisor: &mut ExecutionSupervisor<MemoryExecutionJournal>,
    ticket: InvocationTicket,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    supervisor.command(ExecutionCommand::Enqueue(ticket), now_ms)?;
    Ok(())
}

fn resolve(
    supervisor: &mut ExecutionSupervisor<MemoryExecutionJournal>,
    swarm_id: &str,
    invocation_id: &str,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    supervisor.command(
        ExecutionCommand::ResolveInvocation {
            swarm_id: swarm_id.to_owned(),
            invocation_id: invocation_id.to_owned(),
            resolution: InvocationResolution::Completed,
        },
        now_ms,
    )?;
    Ok(())
}

fn ticket(
    invocation_id: &str,
    swarm_id: &str,
    member_id: &str,
    kind: InvocationKind,
    due_sequence: u64,
) -> InvocationTicket {
    InvocationTicket {
        invocation_id: invocation_id.to_owned(),
        swarm_id: swarm_id.to_owned(),
        member_id: member_id.to_owned(),
        provider: ProviderKind::Controlled,
        configuration_revision: 1,
        kind,
        due_sequence,
    }
}

fn swarm<'a>(
    snapshot: &'a ExecutionSnapshot,
    swarm_id: &str,
) -> Result<&'a SwarmExecutionSnapshot, Box<dyn Error>> {
    snapshot
        .swarms
        .iter()
        .find(|swarm| swarm.swarm_id == swarm_id)
        .ok_or_else(|| format!("missing {swarm_id} execution snapshot").into())
}

fn active_count(snapshot: &ExecutionSnapshot) -> usize {
    snapshot.swarms.iter().map(|swarm| swarm.active.len()).sum()
}
