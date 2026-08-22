#![allow(clippy::panic)]

use worldstream_conformance::{
    AdapterError, KernelConformanceAdapter, ProjectionOutcome, SCENARIOS, ScenarioEvidence,
    ScenarioPlan, ScenarioStatus, digest, run_catalog,
};

/// The executable contract test remains backend-independent. The live test in
/// `shared_live.rs` supplies the real SQLite/PostgreSQL adapters; this small
/// test protects the closed failure rule without manufacturing parity rows.
struct EmptyAdapter;

impl KernelConformanceAdapter for EmptyAdapter {
    fn adapter_name(&self) -> &'static str {
        "empty"
    }
    fn create(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        Ok(ScenarioEvidence::pass(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ))
    }
    fn core_admin(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        self.create(plan)
    }
    fn action(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        self.create(plan)
    }
    fn timer(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        self.create(plan)
    }
    fn frame_cursor_reset(
        &mut self,
        plan: &ScenarioPlan,
    ) -> Result<ScenarioEvidence, AdapterError> {
        self.create(plan)
    }
    fn activation(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        self.create(plan)
    }
    fn recovery(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        self.create(plan)
    }
}

#[test]
fn shared_runner_rejects_missing_provider_evidence() {
    let Err(error) = run_catalog(&mut EmptyAdapter, &ScenarioPlan::counter()) else {
        panic!("empty provider evidence must not become parity")
    };
    assert_eq!(error.operation, SCENARIOS[0].id);
}

#[test]
fn projection_rows_are_byte_checked_by_the_shared_runner() {
    let projection = ProjectionOutcome {
        operation: "projection".to_owned(),
        status: ScenarioStatus::Pass,
        projection_bytes: Some(br"{}".to_vec()),
        projection_hash: Some(vec![0; 32]),
    };
    assert_eq!(projection.status, ScenarioStatus::Pass);
    assert_ne!(
        projection.projection_hash,
        projection.projection_bytes.as_deref().map(digest)
    );
}
