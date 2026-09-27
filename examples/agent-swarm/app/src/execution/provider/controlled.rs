use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ConfigurationResolution, InvocationOutput, InvocationRequest, OutputContract,
    PreparedInvocation, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderInspection,
    ProviderKind, ProviderProbe, SessionSelection, base_prepared, direct_api_environment,
    inspect_contract,
};

/// Deterministic behavior selected only by controlled fixtures.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlledBehavior {
    #[default]
    Normal,
    MismatchModel,
    MismatchEffort,
    Unreported,
    Delayed,
}

impl ControlledBehavior {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::MismatchModel => "mismatch-model",
            Self::MismatchEffort => "mismatch-effort",
            Self::Unreported => "unreported",
            Self::Delayed => "delayed",
        }
    }
}

/// Deterministic provider used for ordinary CI and recovery tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlledAdapter;

impl ControlledAdapter {
    /// Prepares one deterministic invocation with a selected fixture behavior.
    ///
    /// # Errors
    /// Rejects invalid requests or capabilities that do not describe this
    /// exact controlled executable.
    pub fn prepare_with_behavior(
        &self,
        request: &InvocationRequest,
        capabilities: &ProviderCapabilities,
        behavior: ControlledBehavior,
    ) -> Result<PreparedInvocation, ProviderError> {
        request.validate()?;
        let mut arguments = vec![
            "invoke".to_owned(),
            "--invocation-id".to_owned(),
            request.invocation_id.clone(),
            "--model".to_owned(),
            request.model.clone(),
            "--behavior".to_owned(),
            behavior.as_str().to_owned(),
        ];
        if let Some(effort) = &request.effort {
            arguments.push("--effort".to_owned());
            arguments.push(effort.clone());
        }
        match &request.session {
            SessionSelection::Fresh { requested_id } => {
                arguments.push("--new-session".to_owned());
                if let Some(session_id) = requested_id {
                    arguments.push(session_id.clone());
                }
            }
            SessionSelection::Resume { session_id } => {
                arguments.push("--resume".to_owned());
                arguments.push(session_id.clone());
            }
        }
        let mut environment_set = BTreeMap::new();
        environment_set.insert("WORLDSTREAM_CONTROLLED_PROVIDER".to_owned(), "1".to_owned());
        let mut prepared = base_prepared(
            ProviderKind::Controlled,
            request,
            capabilities,
            arguments,
            direct_api_environment(),
            environment_set,
            OutputContract::ControlledJsonLines,
        )?;
        if behavior == ControlledBehavior::Unreported {
            prepared.resolution = ConfigurationResolution::PendingReport;
        }
        Ok(prepared)
    }
}

impl ProviderAdapter for ControlledAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Controlled
    }

    fn inspect(&self, probe: &dyn ProviderProbe, executable: &Path) -> ProviderInspection {
        let mut inspection = inspect_contract(
            ProviderKind::Controlled,
            probe,
            executable,
            &["--version"],
            &["--help"],
            |version, help| {
                let version = version
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("worldstream-controlled-worker "))?
                    .to_owned();
                let complete = [
                    "--invocation-id",
                    "--model",
                    "--effort",
                    "--new-session",
                    "--resume",
                ]
                .iter()
                .all(|needle| help.contains(needle));
                Some((version, complete, complete, complete))
            },
        );
        if let Some(capabilities) = &mut inspection.capabilities {
            capabilities.reports_effective_configuration = true;
            capabilities.delegation_contained = true;
            capabilities.native_cancellation_qualified = true;
            capabilities.resource_confinement_qualified = true;
            inspection.blocker = capabilities.first_blocker(true);
        }
        inspection
    }

    fn prepare(
        &self,
        request: &InvocationRequest,
        capabilities: &ProviderCapabilities,
    ) -> Result<PreparedInvocation, ProviderError> {
        self.prepare_with_behavior(request, capabilities, ControlledBehavior::Normal)
    }

    fn decode(
        &self,
        prepared: &PreparedInvocation,
        stdout: &[u8],
    ) -> Result<InvocationOutput, ProviderError> {
        if prepared.output_contract != OutputContract::ControlledJsonLines {
            return Err(ProviderError::InvalidOutput);
        }
        let text = std::str::from_utf8(stdout).map_err(|_| ProviderError::InvalidOutput)?;
        let mut reported_model = None;
        let mut reported_effort = None;
        let mut session_id = None;
        let mut result = None;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let event: Value =
                serde_json::from_str(line).map_err(|_| ProviderError::InvalidOutput)?;
            match event.get("type").and_then(Value::as_str) {
                Some("configuration") => {
                    reported_model = event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    reported_effort = match event.get("effort") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(value)) => Some(value.clone()),
                        Some(_) => return Err(ProviderError::InvalidOutput),
                    };
                    session_id = event
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                Some("result") => {
                    result = event.get("text").and_then(Value::as_str).map(str::to_owned);
                }
                _ => {}
            }
        }
        Ok(InvocationOutput {
            invocation_id: prepared.invocation_id.clone(),
            reported_model: reported_model.ok_or(ProviderError::InvalidOutput)?,
            reported_effort,
            session_id,
            text: result.ok_or(ProviderError::InvalidOutput)?,
        })
    }
}
