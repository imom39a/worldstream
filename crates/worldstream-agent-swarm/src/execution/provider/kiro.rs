use std::path::Path;

use super::{
    InvocationOutput, InvocationRequest, PreparedInvocation, ProviderAdapter, ProviderBlocker,
    ProviderCapabilities, ProviderError, ProviderInspection, ProviderKind, ProviderProbe,
    inspect_contract,
};

/// Conservative Kiro CLI adapter.
///
/// Current official headless CLI documentation requires an API key and provides
/// bounded JSON-lines output, effort, session resume, and allow-listed tools,
/// but no invocation-scoped model argument. A custom-agent profile can select a
/// model, but the CLI does not report a documented effective
/// model/effort/session record. Consequently discovery describes the installed
/// controls while execution remains blocked instead of inferring configuration
/// from requested values or falling back from subscription login to an API key.
#[derive(Clone, Copy, Debug, Default)]
pub struct KiroAdapter;

impl ProviderAdapter for KiroAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kiro
    }

    fn inspect(&self, probe: &dyn ProviderProbe, executable: &Path) -> ProviderInspection {
        let mut inspection = inspect_contract(
            ProviderKind::Kiro,
            probe,
            executable,
            &["--version"],
            &["chat", "--help"],
            |version, help| {
                let version = version.lines().next()?.trim();
                let headless = help.contains("--no-interactive")
                    && help.contains("--output-format")
                    && help.contains("stream-json")
                    && help.contains("--agent-engine");
                let explicit_effort = help.contains("--effort");
                let session_reuse = help.contains("--resume-id");
                (!version.is_empty() && headless)
                    .then(|| (version.to_owned(), false, explicit_effort, session_reuse))
            },
        );
        inspection.blocker = Some(if inspection.capabilities.is_some() {
            ProviderBlocker::ModelControlUnavailable
        } else {
            ProviderBlocker::UnsupportedVersion
        });
        inspection
    }

    fn prepare(
        &self,
        _request: &InvocationRequest,
        _capabilities: &ProviderCapabilities,
    ) -> Result<PreparedInvocation, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    fn decode(
        &self,
        _prepared: &PreparedInvocation,
        _stdout: &[u8],
    ) -> Result<InvocationOutput, ProviderError> {
        Err(ProviderError::Unsupported)
    }
}
