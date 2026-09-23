//! Stable noninteractive native smoke path.

use std::path::Path;

use crossterm::event::Event;
use ratatui::{Terminal, backend::TestBackend};
use serde::Serialize;
use thiserror::Error;

use crate::{
    AcceptanceCriterion, CreateSwarm, MemberConfiguration, ProviderConfigurationState,
    SwarmApplication,
    fixture::FixtureFileBackend,
    tui::{Command, Input, TerminalState, input_from_event, render},
};

#[derive(Debug, Error)]
#[error("agent swarm smoke test failed")]
pub struct SmokeError;

#[derive(Serialize)]
struct SmokeReceipt<'a> {
    status: &'a str,
    backend: &'a str,
    rooms: usize,
    reopened: bool,
    rendered_sizes: [&'a str; 2],
}

/// Runs the stable fixture-only native smoke contract.
///
/// # Errors
///
/// Returns [`SmokeError`] when the state directory is unavailable or any
/// create, reopen, identity, input, or terminal-render check fails.
pub fn run(state_dir: &Path) -> Result<String, SmokeError> {
    if !state_dir.is_dir() {
        return Err(SmokeError);
    }
    let state_dir = state_dir.canonicalize().map_err(|_| SmokeError)?;
    let store_path = state_dir.join("fixture-swarms.json");
    let mut app = SwarmApplication::new(FixtureFileBackend::new(store_path.clone()));
    let first = app
        .create(request(&state_dir, "Prepare a fixture report"))
        .map_err(|_| SmokeError)?;
    drop(app);

    let mut reopened_app = SwarmApplication::new(FixtureFileBackend::new(store_path));
    let reopened = reopened_app.open(&first.swarm_id).map_err(|_| SmokeError)?;
    if reopened.room_id != first.room_id || reopened.goal != first.goal {
        return Err(SmokeError);
    }
    let second = reopened_app
        .create(request(&state_dir, "Prepare a separate fixture report"))
        .map_err(|_| SmokeError)?;
    if second.room_id == first.room_id || reopened_app.list().map_err(|_| SmokeError)?.len() != 2 {
        return Err(SmokeError);
    }

    let mut state = TerminalState::default();
    if state.handle(&Input::Down, reopened.roster.len()) != Command::None
        || state.selected_member != 1
        || state.handle(&Input::Open, reopened.roster.len()) != Command::OpenSelected
    {
        return Err(SmokeError);
    }
    render_at(&mut state, &reopened, 100, 30)?;
    render_at(&mut state, &reopened, 72, 20)?;
    render_at(&mut state, &reopened, 48, 16)?;
    let paste = "Résumé 世界".to_owned();
    if input_from_event(&Event::Paste(paste.clone())) != Some(Input::Paste { text: paste }) {
        return Err(SmokeError);
    }

    serde_json::to_string(&SmokeReceipt {
        status: "ok",
        backend: "fixture_only",
        rooms: 2,
        reopened: true,
        rendered_sizes: ["100x30", "72x20"],
    })
    .map_err(|_| SmokeError)
}

fn render_at(
    state: &mut TerminalState,
    swarm: &crate::SwarmView,
    width: u16,
    height: u16,
) -> Result<(), SmokeError> {
    state.handle(&Input::Resize { width, height }, swarm.roster.len());
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).map_err(|_| SmokeError)?;
    terminal
        .draw(|frame| render(frame, state, swarm))
        .map_err(|_| SmokeError)?;
    let content = terminal.backend().buffer().content();
    if !content.iter().any(|cell| cell.symbol() == "A") {
        return Err(SmokeError);
    }
    Ok(())
}

fn request(state_dir: &Path, goal: &str) -> CreateSwarm {
    CreateSwarm {
        goal: goal.to_owned(),
        constraints: vec!["Use only supplied fixture material".to_owned()],
        acceptance_criteria: vec![AcceptanceCriterion {
            text: "The report is inspectable after reopening".to_owned(),
        }],
        working_area: state_dir.to_owned(),
        roster: vec![
            fixture_member("analyst-a", "Analyst A", "codex", "fixture-codex"),
            fixture_member("reviewer-b", "Reviewer B", "claude", "fixture-claude"),
        ],
        progress_review_interval_seconds: 300,
        correction_failure_limit: 3,
    }
}

fn fixture_member(key: &str, label: &str, provider: &str, model: &str) -> MemberConfiguration {
    MemberConfiguration {
        member_key: key.to_owned(),
        label: label.to_owned(),
        provider: provider.to_owned(),
        requested_model: model.to_owned(),
        requested_effort: Some("fixture".to_owned()),
        configuration_revision: 1,
        moving_alias_acknowledged: true,
        configuration_state: ProviderConfigurationState::FixtureUnavailable,
    }
}
