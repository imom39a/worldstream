//! Axum shell exposing only bounded operator probes.

mod args;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;
use thiserror::Error;
use worldstream_protocol::{ErrorCode, ErrorEnvelope};
use worldstream_runtime::{
    CompatibilitySummary, EffectiveConfig, ManifestError, StorageProfile, embedded_manifest,
};

pub use args::CommonConfigArgs;

/// Immutable state for the operator-only process shell.
#[derive(Clone, Debug)]
pub struct OperatorState {
    config: EffectiveConfig,
    compatibility: CompatibilitySummary,
}

impl OperatorState {
    /// Builds state from validated config and the embedded manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if the embedded manifest is invalid or its product
    /// version differs from this binary package.
    pub fn new(config: EffectiveConfig) -> Result<Self, ServerError> {
        let compatibility = embedded_manifest()?.summary();
        if compatibility.contracts.product != env!("CARGO_PKG_VERSION") {
            return Err(ServerError::BuildVersionMismatch {
                package: env!("CARGO_PKG_VERSION"),
                manifest: compatibility.contracts.product.clone(),
            });
        }
        Ok(Self {
            config,
            compatibility,
        })
    }
}

/// Builds the complete HTTP surface for this pre-storage milestone.
pub fn operator_router(state: OperatorState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version))
        .with_state(state)
}

async fn healthz() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse { status: "ok" }))
}

async fn readyz() -> (StatusCode, Json<ErrorEnvelope>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorEnvelope::new(
            ErrorCode::StorageNotInitialized,
            "storage is not initialized in the repository bootstrap milestone",
            false,
        )),
    )
}

async fn version(State(state): State<OperatorState>) -> Json<VersionResponse> {
    let contracts = &state.compatibility.contracts;
    Json(VersionResponse {
        product_build: ProductBuild {
            product: contracts.product.clone(),
            binary: "worldstreamd",
            build_version: contracts.product.clone(),
            source_revision: option_env!("WORLDSTREAM_BUILD_REVISION").unwrap_or("unrecorded"),
        },
        wire: contracts.wire.clone(),
        config: contracts.config,
        storage_schema: contracts.storage_schema,
        core_schema_version: contracts.core_schema_version.clone(),
        hash_suite: contracts.hash_suite.clone(),
        manifest: state.compatibility,
        engine: EngineVersion {
            profile: state.config.storage.profile,
            status: "not_initialized",
            exact_identity: None,
        },
    })
}

/// Liveness response that deliberately contains no storage status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    /// Event-loop/process status.
    pub status: &'static str,
}

/// Manifest-backed process build details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProductBuild {
    /// Product contract version from the manifest.
    pub product: String,
    /// Binary package name.
    pub binary: &'static str,
    /// Build version, intentionally sourced from the manifest contract.
    pub build_version: String,
    /// Optional build-system source revision, never guessed at runtime.
    pub source_revision: &'static str,
}

/// Truthful engine state before a storage crate exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EngineVersion {
    /// Startup-selected profile.
    pub profile: StorageProfile,
    /// Initialization state.
    pub status: &'static str,
    /// Exact engine identity once verified; absent in this milestone.
    pub exact_identity: Option<String>,
}

/// Complete operator `/version` response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VersionResponse {
    /// Product and build identity.
    pub product_build: ProductBuild,
    /// Wire version.
    pub wire: String,
    /// Config version.
    pub config: u32,
    /// Storage schema version.
    pub storage_schema: u32,
    /// Core state schema identifier.
    pub core_schema_version: String,
    /// Hash suite identifier.
    pub hash_suite: String,
    /// Embedded manifest identity and pinned engines.
    pub manifest: CompatibilitySummary,
    /// Selected but deliberately uninitialized engine.
    pub engine: EngineVersion,
}

/// Process-shell construction failures.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Embedded compatibility validation failed.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// Cargo package and canonical manifest product versions diverged.
    #[error("binary package version {package} differs from manifest product {manifest}")]
    BuildVersionMismatch {
        package: &'static str,
        manifest: String,
    },
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use worldstream_protocol::{ErrorCode, ErrorEnvelope};
    use worldstream_runtime::EffectiveConfig;

    use super::{OperatorState, VersionResponse, operator_router};

    #[tokio::test]
    async fn health_is_liveness_only_and_ready_is_truthful() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"));
        let app = operator_router(state);

        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(health.status(), 200);

        let ready = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(ready.status(), 503);
        let body = ready
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::StorageNotInitialized);
    }

    #[tokio::test]
    async fn version_is_manifest_backed_and_engine_is_uninitialized() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"));
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("version JSON: {error}"));
        assert_eq!(value["product_build"]["product"], "0.1.0");
        assert_eq!(value["product_build"]["binary"], "worldstreamd");
        assert_eq!(value["wire"], "0.1");
        assert_eq!(
            value["manifest"]["schema"],
            "worldstream/storage-compatibility-manifest/v1"
        );
        assert_eq!(value["manifest"]["release_ready"], false);
        assert_eq!(value["engine"]["status"], "not_initialized");
        assert!(value["engine"]["exact_identity"].is_null());

        let _: Option<VersionResponse> = None;
    }
}
