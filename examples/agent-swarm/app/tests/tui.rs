use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use worldstream_agent_swarm::{
    AcceptanceCriterion, MemberConfiguration, ProviderConfigurationState, SwarmActor, SwarmId,
    SwarmObservation, SwarmView,
    tui::{Command, Input, TerminalState, input_from_event, render, render_observed},
};

fn view() -> SwarmView {
    SwarmView {
        swarm_id: SwarmId::new("fixture-one".to_owned())
            .unwrap_or_else(|error| unreachable!("id: {error}")),
        room_id: "fixture-room-one".to_owned(),
        goal: "Write a report".to_owned(),
        constraints: vec!["Use sources".to_owned()],
        acceptance_criteria: vec![AcceptanceCriterion {
            text: "Cited".to_owned(),
        }],
        working_area: std::env::temp_dir(),
        roster: vec![MemberConfiguration {
            member_key: "fixture-a".to_owned(),
            label: "Fixture A".to_owned(),
            provider: "codex".to_owned(),
            requested_model: "fixture-model".to_owned(),
            requested_effort: None,
            configuration_revision: 1,
            moving_alias_acknowledged: true,
            configuration_state: ProviderConfigurationState::ResolutionUnreported,
        }],
        progress_review_interval_seconds: 300,
        correction_failure_limit: 3,
        source_label: "FIXTURE ONLY — not authoritative WorldStream state".to_owned(),
    }
}

fn observation() -> SwarmObservation {
    SwarmObservation {
        swarm: view(),
        actor: SwarmActor::HumanCoordinator,
        member_id: "fixture-human".to_owned(),
        room_seq: 42,
        authoritative_state_hash: format!("blake3:{}", "a".repeat(64)),
        action_offers: Vec::new(),
        activity: serde_json::json!({
            "lifecycle": "executing",
            "work_items": [{"id":"work-a"}, {"id":"work-b"}],
            "contributions": [{"id":"contribution-a", "artifact": {
                "local_path":"reports/source-a.md", "digest":"blake3:artifact-a"
            }}],
            "candidates": [{"id":"candidate-a"}],
            "checks": [{"id":"check-a"}],
            "reviews": [{"id":"review-a"}],
            "results": [],
            "directions": [{"id":"direction-a"}],
            "problems": [{"id":"problem-a"}],
            "progress_review_obligations": [{"id":"progress-a"}]
        }),
    }
}

#[test]
fn native_terminal_events_map_to_inputs() {
    for (code, expected) in [
        (KeyCode::Up, Input::Up),
        (KeyCode::Down, Input::Down),
        (KeyCode::Enter, Input::Open),
        (KeyCode::Char('n'), Input::New),
        (KeyCode::Char('r'), Input::Refresh),
        (KeyCode::Char('a'), Input::StageNextAction),
        (KeyCode::Char('g'), Input::BeginSuggestion),
        (KeyCode::Char('d'), Input::BeginDirection),
        (KeyCode::Char('c'), Input::BeginConfiguration),
        (KeyCode::Char('e'), Input::BeginExecutionPolicy),
        (KeyCode::Char('i'), Input::InspectArtifact),
        (KeyCode::Tab, Input::ToggleTarget),
        (KeyCode::Backspace, Input::Backspace),
        (KeyCode::Char('l'), Input::StageNextPolicy),
        (KeyCode::Char('p'), Input::Pause),
        (KeyCode::Char('s'), Input::Stop),
        (KeyCode::Char('u'), Input::Resume),
        (KeyCode::Char('q'), Input::Quit),
        (KeyCode::Esc, Input::Home),
    ] {
        assert_eq!(
            input_from_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE))),
            Some(expected)
        );
    }
    assert_eq!(
        input_from_event(&Event::Resize(72, 20)),
        Some(Input::Resize {
            width: 72,
            height: 20
        })
    );
    assert_eq!(
        input_from_event(&Event::Paste("Résumé 世界".to_owned())),
        Some(Input::Paste {
            text: "Résumé 世界".to_owned()
        })
    );
}

#[test]
fn native_key_releases_and_unsupported_keys_are_ignored() {
    let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    assert_eq!(input_from_event(&Event::Key(release)), None);
    assert_eq!(
        input_from_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('界'),
            KeyModifiers::NONE
        ))),
        Some(Input::Text {
            text: "界".to_owned()
        })
    );
}

#[test]
fn input_bounds_selection_and_detaches_explicitly() {
    let mut state = TerminalState::default();
    assert_eq!(state.handle(&Input::Down, 1), Command::None);
    assert_eq!(state.selected_member, 0);
    assert_eq!(state.handle(&Input::Quit, 1), Command::Detach);
}

#[test]
fn dense_dashboard_renders_at_bounded_sizes() {
    for (width, height) in [(100, 30), (72, 20)] {
        let backend = TestBackend::new(width, height);
        let mut terminal =
            Terminal::new(backend).unwrap_or_else(|error| unreachable!("terminal: {error}"));
        terminal
            .draw(|frame| render(frame, &TerminalState::default(), &view()))
            .unwrap_or_else(|error| unreachable!("render: {error}"));
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("AGENT SWARM"));
        assert!(rendered.contains("FIXTURE ONLY"));
        assert!(rendered.contains("resolution unreported"));
        if width == 100 {
            assert!(rendered.contains("r1 · alias:ack"));
        }
        assert!(rendered.contains("acceptance"));
        assert!(rendered.contains("Cited"));
    }
}

#[test]
fn observed_dashboard_separates_room_facts_from_process_activity() {
    let backend = TestBackend::new(100, 30);
    let mut terminal =
        Terminal::new(backend).unwrap_or_else(|error| unreachable!("terminal: {error}"));
    terminal
        .draw(|frame| render_observed(frame, &TerminalState::default(), &observation()))
        .unwrap_or_else(|error| unreachable!("render: {error}"));
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("AUTHORITATIVE ROOM WORKFLOW"));
    assert!(rendered.contains("head 42"));
    assert!(rendered.contains("work 2"));
    assert!(rendered.contains("reports/source-a.md"));
    assert!(rendered.contains("SUPERVISED EXECUTION"));
    assert!(rendered.contains("execution daemon not attached"));
    assert!(rendered.contains("does not imply a worker"));
}

#[test]
fn authoritative_detail_surfaces_workflow_lineage_and_bounded_artifact_metadata() {
    let mut observation = observation();
    observation.activity = serde_json::json!({
        "lifecycle":"executing",
        "roster":[{
            "member_key":"fixture-a",
            "member_id":"member-fixture-a"
        }],
        "work_items":[{
            "work_id":"work-current",
            "revision":3,
            "kind":"integration",
            "dependency_ids":["work-source-a","work-source-b"],
            "owner_member_id":"member-fixture-a",
            "status":"claimed"
        }],
        "contributions":[],
        "candidates":[{
            "candidate_id":"candidate-current",
            "version":2,
            "work_id":"work-current",
            "artifact":{
                "artifact_id":"patch-current",
                "local_path":"patches/fix-世界.diff",
                "digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "media_type":"text/x-diff"
            }
        }],
        "checks":[{
            "check_id":"criterion-1",
            "revision":1,
            "status":"passed",
            "criterion":"tests pass",
            "evidence_refs":[{
                "artifact":{
                    "artifact_id":"check-evidence",
                    "local_path":"evidence/criterion-1.json",
                    "digest":"blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "media_type":"application/vnd.worldstream.agent-swarm-check-evidence+json;version=1"
                }
            }]
        }],
        "reviews":[{
            "review_id":"review-current",
            "revision":4,
            "verdict":"changes_requested",
            "reviewer_member_id":"member-reviewer",
            "finding_ids":["finding-current"]
        }],
        "findings":[{
            "finding_id":"finding-current",
            "revision":2,
            "severity":"blocking",
            "status":"unresolved",
            "corrective_work_id":"work-correction",
            "summary":"Regression evidence is incomplete."
        }],
        "problems":[{
            "problem_id":"problem-current",
            "revision":5,
            "status":"escalated",
            "failed_corrections":3,
            "summary":"Three corrections did not produce evidence.",
            "recorded_correction_attempt_ids":["attempt-1","attempt-2","attempt-3"],
            "evidence_refs":["check:criterion-1:1"]
        }]
    });
    let state = TerminalState {
        workflow_detail: true,
        ..TerminalState::default()
    };
    let backend = TestBackend::new(180, 44);
    let mut terminal =
        Terminal::new(backend).unwrap_or_else(|error| unreachable!("terminal: {error}"));
    terminal
        .draw(|frame| render_observed(frame, &state, &observation))
        .unwrap_or_else(|error| unreachable!("render: {error}"));
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("AUTHORITATIVE ROOM DETAIL"));
    assert!(rendered.contains("work work-current r3 · integration · owner member-fixture-a"));
    assert!(rendered.contains("dependencies: work-source-a,work-source-b"));
    assert!(rendered.contains("latest candidate candidate-current v2"));
    assert!(rendered.contains("patches/fix-世"));
    assert!(rendered.contains("界 .diff"));
    assert!(rendered.contains("text/x-diff"));
    assert!(rendered.contains("latest check criterion-1 r1 · passed"));
    assert!(rendered.contains("evidence/criterion-1.json"));
    assert!(rendered.contains("latest review review-current r4 · changes_requested"));
    assert!(rendered.contains("latest finding finding-current r2 · blocking/unresolved"));
    assert!(rendered.contains("latest problem problem-current r5 · escalated"));
    assert!(rendered.contains("failed corrections 3"));
    assert!(rendered.contains("correction attempts: attempt-1,attempt-2,attempt-3"));
    assert!(rendered.contains("evidence: check:criterion-1:1"));
}
