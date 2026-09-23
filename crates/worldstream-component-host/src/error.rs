use thiserror::Error;

/// Stable, redacted failure raised before a portable Pack can enter Core.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ComponentHostErrorV1 {
    #[error("portable Activity Pack compilation is temporarily saturated")]
    CompileConcurrencyLimit,
    #[error("portable Activity Pack Component bytes are malformed or unsupported")]
    ComponentRejected,
    #[error("portable Activity Pack disposable compilation cache is unavailable")]
    ComponentCacheUnavailable,
    #[error("portable Activity Pack Component imports a forbidden capability")]
    ForbiddenImport,
    #[error("portable Activity Pack Component exports do not match the frozen contract")]
    ExportContractMismatch,
    #[error("portable Activity Pack descriptor callback failed")]
    DescriptorCallbackFailed,
    #[error("portable Activity Pack descriptor content differs from the verified bundle")]
    DescriptorContentMismatch,
}
