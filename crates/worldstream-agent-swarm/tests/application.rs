use std::path::Path;

use worldstream_agent_swarm::{
    AcceptanceCriterion, ApplicationError, BackendError, CreateSwarm,
    DEFAULT_CORRECTION_FAILURE_LIMIT, DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS,
    MemberConfiguration, ProviderConfigurationState, SwarmActor, SwarmApplication, SwarmBackend,
    ValidatedCreateSwarm, ValidationError, fixture::FixtureFileBackend,
};

fn request(area: &Path, goal: &str) -> CreateSwarm {
    CreateSwarm {
        goal: goal.to_owned(),
        constraints: vec!["Stay within scope".to_owned()],
        acceptance_criteria: vec![AcceptanceCriterion {
            text: "Result is visible".to_owned(),
        }],
        working_area: area.to_owned(),
        roster: vec![MemberConfiguration {
            member_key: "fixture-a".to_owned(),
            label: "Fixture A".to_owned(),
            provider: "codex".to_owned(),
            requested_model: "fixture-model".to_owned(),
            requested_effort: Some("fixture".to_owned()),
            configuration_revision: 1,
            moving_alias_acknowledged: true,
            configuration_state: ProviderConfigurationState::FixtureUnavailable,
        }],
        progress_review_interval_seconds: 300,
        correction_failure_limit: 3,
    }
}

#[test]
fn fixture_backend_never_manufactures_participant_authority() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let path = directory.path().join("fixture.json");
    let mut application = SwarmApplication::new(FixtureFileBackend::new(path));
    let created = application
        .create(request(directory.path(), "Authority boundary"))
        .unwrap_or_else(|error| unreachable!("create: {error}"));

    assert!(matches!(
        application.observe(&created.swarm_id, &SwarmActor::HumanCoordinator),
        Err(ApplicationError::Backend(BackendError::Unsupported))
    ));
}

#[test]
fn creates_reopens_and_separates_goals() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let path = directory.path().join("fixture.json");
    let mut app = SwarmApplication::new(FixtureFileBackend::new(path.clone()));
    let first = app
        .create(request(directory.path(), "First goal"))
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    drop(app);

    let mut reopened = SwarmApplication::new(FixtureFileBackend::new(path));
    let view = reopened
        .open(&first.swarm_id)
        .unwrap_or_else(|error| unreachable!("open: {error}"));
    assert_eq!(view.room_id, first.room_id);
    assert!(view.source_label.contains("FIXTURE ONLY"));

    let second = reopened
        .create(request(directory.path(), "Second goal"))
        .unwrap_or_else(|error| unreachable!("second: {error}"));
    assert_ne!(second.room_id, first.room_id);
    let listed = reopened
        .list()
        .unwrap_or_else(|error| unreachable!("list: {error}"));
    assert_eq!(listed.len(), 2);
}

#[test]
fn invalid_request_never_reaches_backend() {
    struct ClosedBackend;
    impl SwarmBackend for ClosedBackend {
        fn create(
            &mut self,
            _request: ValidatedCreateSwarm,
        ) -> Result<worldstream_agent_swarm::SwarmView, BackendError> {
            unreachable!("invalid input reached backend")
        }
        fn list(&self) -> Result<Vec<worldstream_agent_swarm::SwarmSummary>, BackendError> {
            Ok(Vec::new())
        }
        fn open(
            &self,
            _swarm_id: &worldstream_agent_swarm::SwarmId,
        ) -> Result<worldstream_agent_swarm::SwarmView, BackendError> {
            Err(BackendError::NotFound)
        }
    }

    let mut invalid = request(Path::new("relative"), "Goal");
    invalid.acceptance_criteria.clear();
    let Err(error) = SwarmApplication::new(ClosedBackend).create(invalid) else {
        unreachable!("request should fail");
    };
    assert!(matches!(error, ApplicationError::Validation(_)));
}

#[test]
fn progress_review_interval_defaults_validates_and_round_trips() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let mut encoded = serde_json::to_value(request(directory.path(), "Review interval"))
        .unwrap_or_else(|error| unreachable!("serialize: {error}"));
    encoded
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("request is an object"))
        .remove("progress_review_interval_seconds");
    encoded
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("request is an object"))
        .remove("correction_failure_limit");
    let defaulted: CreateSwarm = serde_json::from_value(encoded)
        .unwrap_or_else(|error| unreachable!("deserialize: {error}"));
    assert_eq!(
        defaulted.progress_review_interval_seconds,
        DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS
    );
    assert_eq!(
        defaulted.correction_failure_limit,
        DEFAULT_CORRECTION_FAILURE_LIMIT
    );

    let path = directory.path().join("interval.json");
    let mut custom = request(directory.path(), "Custom interval");
    custom.progress_review_interval_seconds = 37;
    custom.correction_failure_limit = 5;
    let created = SwarmApplication::new(FixtureFileBackend::new(path))
        .create(custom)
        .unwrap_or_else(|error| unreachable!("create: {error}"));
    assert_eq!(created.progress_review_interval_seconds, 37);
    assert_eq!(created.correction_failure_limit, 5);

    let mut invalid = request(directory.path(), "Invalid interval");
    invalid.progress_review_interval_seconds = 0;
    assert!(matches!(
        ValidatedCreateSwarm::try_from(invalid),
        Err(ValidationError::InvalidProgressReviewInterval)
    ));

    let mut invalid = request(directory.path(), "Invalid correction limit");
    invalid.correction_failure_limit = 0;
    assert!(matches!(
        ValidatedCreateSwarm::try_from(invalid),
        Err(ValidationError::InvalidCorrectionFailureLimit)
    ));
    let mut excessive = request(directory.path(), "Excessive correction limit");
    excessive.correction_failure_limit = 65;
    assert!(matches!(
        ValidatedCreateSwarm::try_from(excessive),
        Err(ValidationError::InvalidCorrectionFailureLimit)
    ));
}

#[test]
fn roster_bound_matches_the_pack_worker_limit() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let mut oversized = request(directory.path(), "Too many workers");
    let template = oversized.roster[0].clone();
    oversized.roster = (0..17)
        .map(|index| MemberConfiguration {
            member_key: format!("worker-{index}"),
            label: format!("Worker {index}"),
            ..template.clone()
        })
        .collect();
    assert!(matches!(
        ValidatedCreateSwarm::try_from(oversized),
        Err(ValidationError::InvalidRosterSize)
    ));
}

#[test]
fn moving_model_alias_requires_creation_time_acknowledgement() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let mut unacknowledged = request(directory.path(), "Explicit alias choice");
    unacknowledged.roster[0].requested_model = "latest".to_owned();
    unacknowledged.roster[0].moving_alias_acknowledged = false;
    assert!(matches!(
        ValidatedCreateSwarm::try_from(unacknowledged),
        Err(ValidationError::InvalidMember)
    ));

    let mut acknowledged = request(directory.path(), "Explicit alias choice");
    acknowledged.roster[0].requested_model = "latest".to_owned();
    acknowledged.roster[0].moving_alias_acknowledged = true;
    assert!(ValidatedCreateSwarm::try_from(acknowledged).is_ok());
}
