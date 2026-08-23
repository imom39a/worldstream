//! Offline storage operations used by the shipped operator binary.
//!
//! Native `SQLite` verification and full `WorldStream` restore verification are
//! deliberately reported separately. A native-only check is useful
//! diagnostics, but it is never presented as restore readiness.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use worldstream_backup::native_sqlite::{
    NativeSqliteCaptureWitnessV1, NativeSqliteFileIdentityV1, NativeSqliteLimits,
    NativeSqliteRestoreEvidenceV1, NativeSqliteTransferReportV1, NativeSqliteVerificationReportV1,
    backup_file, extract_restore_evidence_retained, restore_file, verify_retained_file,
};
use worldstream_backup::{
    BackendProfileV1, DigestV1, NativeSqliteBackupEnvelopeV1, NativeSqliteEnvelopeCoverageV1,
    NativeSqliteEnvelopeOriginV1, NativeSqliteNativeWitnessV1, VerifierLimits,
    native_evidence_digest, native_membership_digest, verify_native_restore,
};
#[cfg(windows)]
use worldstream_runtime::create_owner_only_renameable_file;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

const RESULT_SCHEMA: &str = "worldstream/operator-sqlite-result/v1";
const MAX_ENVELOPE_BYTES: usize = 64 * 1024 * 1024;
const MAX_NATIVE_BACKUP_BYTES: u64 = 1024 * 1024 * 1024 * 1024;

#[cfg(unix)]
pub(crate) type ArtifactIdentity = (u64, u64);

#[cfg(windows)]
pub(crate) type ArtifactIdentity = fs_id::FileID;

/// A redacted result from a shipped `SQLite` operator operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SqliteOperatorResultV1 {
    /// Stable output schema.
    pub schema: &'static str,
    /// Successful operations always report `ok`.
    pub status: &'static str,
    /// Operation performed by the native adapter.
    pub operation: &'static str,
    /// Exact bundled `SQLite` engine observed by the native verifier.
    pub engine_version: String,
    /// Native pages copied for backup/restore; absent for verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u64>,
    /// Disposition of the bounded native `SQLite` verifier.
    pub native_verifier: &'static str,
    /// `pass` only after the full `WorldStream` native-restore verifier is Ready.
    pub semantic_verifier: &'static str,
    /// Number of canonical Rooms inspected.
    pub room_count: usize,
    /// Number of canonical Transitions inspected.
    pub transition_count: usize,
    /// Number of operational records inspected across bounded ledgers.
    pub operational_record_count: usize,
    /// Digest sealed into the companion envelope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_digest: Option<String>,
    /// Digest of the exact published canonical envelope bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_file_digest: Option<String>,
    /// Size of the exact published canonical envelope bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_bytes: Option<usize>,
    /// Bounded full-verifier diagnostic count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_diagnostic_count: Option<usize>,
    /// Canonical storage identifier for the exact output object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_storage_id: Option<String>,
    /// Canonical file identifier for the exact output object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file_id: Option<String>,
    /// Canonical storage identifier for the exact companion envelope object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_storage_id: Option<String>,
    /// Canonical file identifier for the exact companion envelope object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_file_id: Option<String>,
}

/// Closed failures from an offline `SQLite` operator operation.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SqliteOperatorError {
    /// An argument or artifact layout was outside the reviewed contract.
    #[error("SQLite operator argument or artifact layout was rejected: {0}")]
    InvalidArgument(&'static str),
    /// The reviewed native `SQLite` adapter rejected the operation.
    #[error("SQLite native operation failed closed: {0}")]
    Native(&'static str),
    /// The bounded, canonical companion envelope was absent or invalid.
    #[error("SQLite companion envelope failed closed: {0}")]
    Companion(&'static str),
    /// The complete `WorldStream` verifier did not return Ready.
    #[error("SQLite full restore verification rejected the image")]
    VerificationRejected,
    /// The owner-only filesystem contract could not be established.
    #[error("SQLite protected artifact operation failed closed: {0}")]
    Filesystem(&'static str),
    /// Exact-handle scrubbing could not establish a complete cleanup result.
    #[error("SQLite operation is incomplete; a protected scrubbed artifact may remain")]
    IncompleteArtifact,
}

/// Runs the complete bounded, read-only native `SQLite` verifier.
///
/// This operation intentionally does not mint a native capture witness, so it
/// reports `semantic_verifier = "not_invoked"` and is not a restore-readiness
/// assertion.
///
/// # Errors
///
/// Returns a closed error when the owner-only database cannot be read or any
/// bounded native verification check rejects it.
pub fn verify_sqlite(database: &Path) -> Result<SqliteOperatorResultV1, SqliteOperatorError> {
    let artifact = RetainedArtifact::open_owner_only(database)?;
    let verification = verified_report(&artifact, "verification")?;
    artifact.revalidate()?;
    Ok(native_only_result(
        "verify",
        None,
        &verification,
        artifact.canonical_identity(),
    ))
}

/// Creates a native backup and a capture-bound, full-verifier-ready envelope.
///
/// `companion` must be canonical owner-only JSON produced by the reviewed
/// envelope serializer. It supplies pack/resource bytes and facts `SQLite` does
/// not persist. `output` and `envelope_path` must both be absent and have
/// owner-only parents.
///
/// # Errors
///
/// Returns a closed error unless the exact native backup and sealed companion
/// envelope can both be published, reopened, and verified as restore-ready.
pub fn backup_sqlite(
    database: &Path,
    output: &Path,
    companion: &Path,
    envelope_path: &Path,
) -> Result<SqliteOperatorResultV1, SqliteOperatorError> {
    let (_, mut envelope) = read_strict_envelope(companion)?;
    let output = prepare_new_artifact(output)?;
    let envelope_path = prepare_new_artifact(envelope_path)?;
    if output == envelope_path {
        return Err(SqliteOperatorError::InvalidArgument(
            "backup and envelope destinations must differ",
        ));
    }

    let transfer =
        backup_file(database, &output).map_err(|_| SqliteOperatorError::Native("online backup"))?;
    let Ok(mut backup_artifact) =
        RetainedArtifact::open_owner_only_expected(&output, Some(transfer.destination_identity()))
    else {
        return Err(SqliteOperatorError::IncompleteArtifact);
    };
    let mut published_envelope = None;

    let result = (|| {
        let native = verified_report(&backup_artifact, "backup verification")?;
        backup_artifact.revalidate()?;
        let evidence = extract_retained_evidence(&backup_artifact, "backup evidence extraction")?;
        bind_and_seal_envelope(&mut envelope, &evidence, transfer.capture_witness())?;
        let semantic_diagnostics =
            verify_ready(&envelope, &evidence, &evidence, transfer.capture_witness())?;
        let envelope_bytes = serde_json::to_vec(&envelope)
            .map_err(|_| SqliteOperatorError::Companion("canonical encoding"))?;
        if envelope_bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(SqliteOperatorError::Companion("envelope byte bound"));
        }
        let artifact = publish_owner_only(&envelope_path, &envelope_bytes)?;
        published_envelope = Some(artifact);

        let published_bytes = envelope_bytes.clone();
        let published: NativeSqliteBackupEnvelopeV1 = decode_strict_json(&published_bytes)?;
        published_envelope
            .as_mut()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?
            .require_exact_bytes(&published_bytes)?;
        published
            .validate(VerifierLimits::default())
            .map_err(|_| SqliteOperatorError::Companion("published envelope seal validation"))?;
        published_envelope
            .as_mut()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?
            .revalidate()?;

        let reread_native = verified_report(&backup_artifact, "backup reread verification")?;
        backup_artifact.revalidate()?;
        let reread_evidence =
            extract_retained_evidence(&backup_artifact, "backup reread evidence extraction")?;
        let reread_diagnostics = verify_ready(
            &published,
            &reread_evidence,
            &reread_evidence,
            transfer.capture_witness(),
        )?;
        if native != reread_native || semantic_diagnostics != reread_diagnostics {
            return Err(SqliteOperatorError::VerificationRejected);
        }
        backup_artifact.revalidate()?;
        let output_identity = backup_artifact.canonical_identity();
        let envelope_identity = published_envelope
            .as_ref()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?
            .canonical_identity();
        Ok(full_result(
            "backup",
            &transfer,
            &reread_native,
            &published,
            &published_bytes,
            reread_diagnostics,
            FullResultIdentities {
                output: output_identity,
                envelope: envelope_identity,
            },
        ))
    })();

    finish_or_scrub(
        result,
        &mut [published_envelope.as_mut(), Some(&mut backup_artifact)],
    )
}

/// Restores an exact native backup and requires its exact sealed envelope to
/// rebuild a Ready full-verifier report before reporting success.
///
/// # Errors
///
/// Returns a closed error unless the admitted backup bytes and exact envelope
/// remain bound throughout restore and the full verifier returns `Ready`.
pub fn restore_sqlite(
    backup: &Path,
    envelope_path: &Path,
    database: &Path,
) -> Result<SqliteOperatorResultV1, SqliteOperatorError> {
    restore_sqlite_inner(backup, envelope_path, database, |_, _| Ok(()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreInputStage {
    BeforeNativeRestore,
    AfterNativeRestore,
    BeforeSnapshotCleanup,
}

fn restore_sqlite_inner<F>(
    backup: &Path,
    envelope_path: &Path,
    database: &Path,
    mut input_hook: F,
) -> Result<SqliteOperatorResultV1, SqliteOperatorError>
where
    F: FnMut(RestoreInputStage, &Path) -> Result<(), SqliteOperatorError>,
{
    let (mut envelope_input, envelope_bytes, envelope) = open_validated_envelope(envelope_path)?;
    let mut backup_input = RetainedInput::open_owner_only(backup)?;
    let backup_fingerprint = backup_input.fingerprint(MAX_NATIVE_BACKUP_BYTES)?;
    let database = prepare_new_artifact(database)?;
    let mut admitted = AdmittedBackupSnapshot::create(
        &mut backup_input,
        backup_fingerprint,
        database.parent().ok_or(SqliteOperatorError::Filesystem(
            "restore destination parent",
        ))?,
    )?;
    let admitted_path = admitted.path().to_owned();
    let mut restored_artifact = None;

    let result = (|| {
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        verified_input_report(&admitted.input, "backup input verification")?;
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        envelope_input.require_exact_bytes(&envelope_bytes, MAX_ENVELOPE_BYTES)?;

        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        input_hook(RestoreInputStage::BeforeNativeRestore, &admitted_path)?;
        let transfer = restore_file(&admitted_path, &database)
            .map_err(|_| SqliteOperatorError::Native("online restore"))?;
        input_hook(RestoreInputStage::AfterNativeRestore, &admitted_path)?;
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        backup_input.require_fingerprint(backup_fingerprint, MAX_NATIVE_BACKUP_BYTES)?;
        envelope_input.require_exact_bytes(&envelope_bytes, MAX_ENVELOPE_BYTES)?;
        restored_artifact = Some(
            RetainedArtifact::open_owner_only_expected(
                &database,
                Some(transfer.destination_identity()),
            )
            .map_err(|_| SqliteOperatorError::IncompleteArtifact)?,
        );

        let restored_artifact_ref = restored_artifact
            .as_ref()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?;
        let native = verified_report(restored_artifact_ref, "restored target verification")?;
        restored_artifact_ref.revalidate()?;
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        let source = extract_input_evidence(&admitted.input, "backup evidence extraction")?;
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        let restored =
            extract_retained_evidence(restored_artifact_ref, "restored evidence extraction")?;
        let diagnostics = verify_ready(&envelope, &source, &restored, transfer.capture_witness())?;

        let (reread_bytes, reread_envelope) =
            reread_validated_envelope(&mut envelope_input, &envelope_bytes)?;
        let restored_artifact_ref = restored_artifact
            .as_ref()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?;
        let reread_native =
            verified_report(restored_artifact_ref, "restored target reread verification")?;
        restored_artifact_ref.revalidate()?;
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        let reread_source =
            extract_input_evidence(&admitted.input, "backup reread evidence extraction")?;
        revalidate_admitted_backup(&mut backup_input, backup_fingerprint, &mut admitted)?;
        let reread_restored = extract_retained_evidence(
            restored_artifact_ref,
            "restored reread evidence extraction",
        )?;
        let reread_diagnostics = verify_ready(
            &reread_envelope,
            &reread_source,
            &reread_restored,
            transfer.capture_witness(),
        )?;
        if native != reread_native || diagnostics != reread_diagnostics {
            return Err(SqliteOperatorError::VerificationRejected);
        }
        backup_input.require_fingerprint(backup_fingerprint, MAX_NATIVE_BACKUP_BYTES)?;
        envelope_input.require_exact_bytes(&envelope_bytes, MAX_ENVELOPE_BYTES)?;
        restored_artifact_ref.revalidate()?;
        envelope_input.revalidate()?;
        Ok(full_result(
            "restore",
            &transfer,
            &reread_native,
            &reread_envelope,
            &reread_bytes,
            reread_diagnostics,
            FullResultIdentities {
                output: restored_artifact_ref.canonical_identity(),
                envelope: envelope_input.canonical_identity(),
            },
        ))
    })();

    let result = match input_hook(RestoreInputStage::BeforeSnapshotCleanup, &admitted_path) {
        Ok(()) => result,
        Err(error) => Err(error),
    };
    let cleanup = admitted.scrub_and_remove();
    let result = if cleanup.is_ok() {
        result
    } else {
        Err(SqliteOperatorError::IncompleteArtifact)
    };
    finish_or_scrub(result, &mut [restored_artifact.as_mut()])
}

fn open_validated_envelope(
    path: &Path,
) -> Result<(RetainedInput, Vec<u8>, NativeSqliteBackupEnvelopeV1), SqliteOperatorError> {
    let mut input = RetainedInput::open_owner_only(path)?;
    let bytes = input.read_bounded(MAX_ENVELOPE_BYTES)?;
    let envelope: NativeSqliteBackupEnvelopeV1 = decode_strict_json(&bytes)?;
    envelope
        .validate(VerifierLimits::default())
        .map_err(|_| SqliteOperatorError::Companion("envelope seal validation"))?;
    Ok((input, bytes, envelope))
}

fn reread_validated_envelope(
    input: &mut RetainedInput,
    expected_bytes: &[u8],
) -> Result<(Vec<u8>, NativeSqliteBackupEnvelopeV1), SqliteOperatorError> {
    let bytes = input.read_bounded(MAX_ENVELOPE_BYTES)?;
    if bytes != expected_bytes {
        return Err(SqliteOperatorError::Companion(
            "envelope changed during restore",
        ));
    }
    let envelope: NativeSqliteBackupEnvelopeV1 = decode_strict_json(&bytes)?;
    input.require_exact_bytes(expected_bytes, MAX_ENVELOPE_BYTES)?;
    envelope
        .validate(VerifierLimits::default())
        .map_err(|_| SqliteOperatorError::Companion("reread envelope seal validation"))?;
    Ok((bytes, envelope))
}

fn bind_and_seal_envelope(
    envelope: &mut NativeSqliteBackupEnvelopeV1,
    evidence: &NativeSqliteRestoreEvidenceV1,
    capture: &NativeSqliteCaptureWitnessV1,
) -> Result<(), SqliteOperatorError> {
    if envelope.manifest.backend != BackendProfileV1::SqliteBundled {
        return Err(SqliteOperatorError::Companion("backend profile"));
    }
    let lineage = evidence
        .deployment_lineage
        .clone()
        .ok_or(SqliteOperatorError::Companion("deployment lineage"))?;
    let epoch = evidence
        .storage_epoch
        .ok_or(SqliteOperatorError::Companion("storage epoch"))?;
    let native_point = capture.native_point().clone();
    let capture_id = match &native_point {
        worldstream_backup::BackendNativePointV1::SqliteOnlineBackup { point_id, .. } => {
            point_id.clone()
        }
        worldstream_backup::BackendNativePointV1::PostgresNative { .. } => {
            return Err(SqliteOperatorError::Companion("native capture profile"));
        }
    };
    let evidence_digest = native_evidence_digest(evidence)
        .map_err(|_| SqliteOperatorError::Companion("native evidence digest"))?;
    if capture.source_evidence_digest() != &evidence_digest
        || capture.target_evidence_digest() != &evidence_digest
    {
        return Err(SqliteOperatorError::Companion("capture evidence binding"));
    }
    let membership_digest = native_membership_digest(&evidence.operational)
        .map_err(|_| SqliteOperatorError::Companion("membership digest"))?;
    let witness = NativeSqliteNativeWitnessV1 {
        backend: BackendProfileV1::SqliteBundled,
        native_point: native_point.clone(),
        deployment_lineage: lineage.clone(),
        storage_epoch: epoch,
        evidence_digest: evidence_digest.clone(),
        membership_digest: membership_digest.clone(),
    };

    envelope.origin = NativeSqliteEnvelopeOriginV1 {
        producer: "worldstream-native-sqlite-adapter".to_owned(),
        capture_id,
        coverage: "online-backup plus exact companion witnesses".to_owned(),
    };
    envelope.manifest.deployment_lineage = lineage;
    envelope.manifest.storage_epoch = epoch;
    envelope.manifest.native_point = native_point;
    envelope.source = witness.clone();
    envelope.restored = witness;
    envelope.coverage = NativeSqliteEnvelopeCoverageV1 {
        tables: envelope.coverage.tables.clone(),
        source_evidence_digest: evidence_digest.clone(),
        restored_evidence_digest: evidence_digest,
        source_membership_digest: membership_digest.clone(),
        restored_membership_digest: membership_digest,
    };
    envelope
        .seal(evidence, evidence, capture, VerifierLimits::default())
        .map_err(|_| SqliteOperatorError::Companion("seal complete companion"))?;
    envelope
        .validate(VerifierLimits::default())
        .map_err(|_| SqliteOperatorError::Companion("sealed envelope validation"))
}

fn verify_ready(
    envelope: &NativeSqliteBackupEnvelopeV1,
    source: &NativeSqliteRestoreEvidenceV1,
    restored: &NativeSqliteRestoreEvidenceV1,
    capture: &NativeSqliteCaptureWitnessV1,
) -> Result<usize, SqliteOperatorError> {
    let evidence = envelope
        .build_native_restore_evidence(source, restored, capture, VerifierLimits::default())
        .map_err(|_| SqliteOperatorError::Companion("build complete restore evidence"))?;
    let report = verify_native_restore(&evidence, VerifierLimits::default());
    if !report.is_ready() {
        return Err(SqliteOperatorError::VerificationRejected);
    }
    Ok(report.diagnostics.len())
}

fn verified_report(
    artifact: &RetainedArtifact,
    operation: &'static str,
) -> Result<NativeSqliteVerificationReportV1, SqliteOperatorError> {
    artifact.revalidate()?;
    let report = verify_retained_file(
        &artifact.path,
        artifact.file()?,
        NativeSqliteLimits::default(),
    )
    .map_err(|_| SqliteOperatorError::Native(operation))?;
    artifact.revalidate()?;
    if !report.canonical_ready || report.diagnostics.iter().any(|item| item.blocking) {
        return Err(SqliteOperatorError::Native(operation));
    }
    Ok(report)
}

fn verified_input_report(
    input: &RetainedInput,
    operation: &'static str,
) -> Result<NativeSqliteVerificationReportV1, SqliteOperatorError> {
    input.revalidate()?;
    let report = verify_retained_file(&input.path, &input.file, NativeSqliteLimits::default())
        .map_err(|_| SqliteOperatorError::Native(operation))?;
    input.revalidate()?;
    if !report.canonical_ready || report.diagnostics.iter().any(|item| item.blocking) {
        return Err(SqliteOperatorError::Native(operation));
    }
    Ok(report)
}

fn extract_retained_evidence(
    artifact: &RetainedArtifact,
    operation: &'static str,
) -> Result<NativeSqliteRestoreEvidenceV1, SqliteOperatorError> {
    artifact.revalidate()?;
    let evidence = extract_restore_evidence_retained(
        &artifact.path,
        artifact.file()?,
        NativeSqliteLimits::default(),
    )
    .map_err(|_| SqliteOperatorError::Native(operation))?;
    artifact.revalidate()?;
    Ok(evidence)
}

fn extract_input_evidence(
    input: &RetainedInput,
    operation: &'static str,
) -> Result<NativeSqliteRestoreEvidenceV1, SqliteOperatorError> {
    input.revalidate()?;
    let evidence =
        extract_restore_evidence_retained(&input.path, &input.file, NativeSqliteLimits::default())
            .map_err(|_| SqliteOperatorError::Native(operation))?;
    input.revalidate()?;
    Ok(evidence)
}

fn native_only_result(
    operation: &'static str,
    page_count: Option<u64>,
    report: &NativeSqliteVerificationReportV1,
    output_identity: (String, String),
) -> SqliteOperatorResultV1 {
    base_result(
        operation,
        page_count,
        report,
        "not_invoked",
        None,
        None,
        None,
        None,
        Some(output_identity),
        None,
    )
}

struct FullResultIdentities {
    output: (String, String),
    envelope: (String, String),
}

fn full_result(
    operation: &'static str,
    transfer: &NativeSqliteTransferReportV1,
    report: &NativeSqliteVerificationReportV1,
    envelope: &NativeSqliteBackupEnvelopeV1,
    envelope_bytes: &[u8],
    diagnostic_count: usize,
    identities: FullResultIdentities,
) -> SqliteOperatorResultV1 {
    base_result(
        operation,
        Some(transfer.page_count),
        report,
        "pass",
        Some(envelope.envelope_digest.as_str().to_owned()),
        Some(DigestV1::hash(envelope_bytes).as_str().to_owned()),
        Some(envelope_bytes.len()),
        Some(diagnostic_count),
        Some(identities.output),
        Some(identities.envelope),
    )
}

#[allow(clippy::too_many_arguments)]
fn base_result(
    operation: &'static str,
    page_count: Option<u64>,
    report: &NativeSqliteVerificationReportV1,
    semantic_verifier: &'static str,
    envelope_digest: Option<String>,
    envelope_file_digest: Option<String>,
    envelope_bytes: Option<usize>,
    semantic_diagnostic_count: Option<usize>,
    output_identity: Option<(String, String)>,
    envelope_identity: Option<(String, String)>,
) -> SqliteOperatorResultV1 {
    let (output_storage_id, output_file_id) = output_identity.unzip();
    let (envelope_storage_id, envelope_file_id) = envelope_identity.unzip();
    SqliteOperatorResultV1 {
        schema: RESULT_SCHEMA,
        status: "ok",
        operation,
        engine_version: report.engine_version.clone(),
        page_count,
        native_verifier: "pass",
        semantic_verifier,
        room_count: report.room_count,
        transition_count: report.transition_count,
        operational_record_count: report
            .timer_count
            .saturating_add(report.frame_count)
            .saturating_add(report.semantic_receipt_count)
            .saturating_add(report.activation_intent_count)
            .saturating_add(report.activation_receipt_count)
            .saturating_add(report.activation_decision_count)
            .saturating_add(report.observation_consequence_count),
        envelope_digest,
        envelope_file_digest,
        envelope_bytes,
        semantic_diagnostic_count,
        output_storage_id,
        output_file_id,
        envelope_storage_id,
        envelope_file_id,
    }
}

fn read_strict_envelope(
    path: &Path,
) -> Result<(Vec<u8>, NativeSqliteBackupEnvelopeV1), SqliteOperatorError> {
    let bytes = read_owner_only_bounded(path, MAX_ENVELOPE_BYTES)?;
    let envelope = decode_strict_json(&bytes)?;
    Ok((bytes, envelope))
}

fn decode_strict_json<T>(bytes: &[u8]) -> Result<T, SqliteOperatorError>
where
    T: DeserializeOwned + Serialize,
{
    let value: T = serde_json::from_slice(bytes)
        .map_err(|_| SqliteOperatorError::Companion("canonical JSON decoding"))?;
    let canonical = serde_json::to_vec(&value)
        .map_err(|_| SqliteOperatorError::Companion("canonical JSON encoding"))?;
    if canonical != bytes {
        return Err(SqliteOperatorError::Companion(
            "noncanonical or unknown JSON fields",
        ));
    }
    Ok(value)
}

fn read_owner_only_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, SqliteOperatorError> {
    validate_owner_only_file(path)
        .map_err(|_| SqliteOperatorError::Filesystem("owner-only input"))?;
    let mut file = File::open(path).map_err(|_| SqliteOperatorError::Filesystem("open input"))?;
    let identity =
        file_identity(&file).map_err(|_| SqliteOperatorError::Filesystem("input identity"))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| SqliteOperatorError::Filesystem("bounded input read"))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(SqliteOperatorError::Companion("envelope byte bound"));
    }
    validate_owner_only_file(path)
        .map_err(|_| SqliteOperatorError::Filesystem("owner-only input reread"))?;
    if path_identity(path).ok() != Some(identity) {
        return Err(SqliteOperatorError::Filesystem("input identity changed"));
    }
    Ok(bytes)
}

fn prepare_new_artifact(path: &Path) -> Result<PathBuf, SqliteOperatorError> {
    let name = path
        .file_name()
        .ok_or(SqliteOperatorError::InvalidArgument("artifact file name"))?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = prepare_data_directory(parent)
        .map_err(|_| SqliteOperatorError::Filesystem("owner-only artifact directory"))?;
    let path = parent.join(name);
    if fs::symlink_metadata(&path).is_ok() {
        return Err(SqliteOperatorError::InvalidArgument(
            "artifact destination already exists",
        ));
    }
    Ok(path)
}

fn publish_owner_only(path: &Path, bytes: &[u8]) -> Result<RetainedArtifact, SqliteOperatorError> {
    publish_owner_only_with_hook(path, bytes, |_| Ok(()))
}

#[allow(clippy::too_many_lines)]
fn publish_owner_only_with_hook<F>(
    path: &Path,
    bytes: &[u8],
    after_sync: F,
) -> Result<RetainedArtifact, SqliteOperatorError>
where
    F: FnOnce(&Path) -> Result<(), SqliteOperatorError>,
{
    let parent = PublicationParent::open(path)
        .map_err(|_| SqliteOperatorError::Filesystem("open envelope parent authority"))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(SqliteOperatorError::InvalidArgument("envelope file name"))?;
    let nonce = random_nonce()?;
    let partial = path.with_file_name(format!(".{name}.{nonce}.partial"));
    let partial_name = partial
        .file_name()
        .ok_or(SqliteOperatorError::InvalidArgument(
            "envelope partial name",
        ))?
        .to_owned();
    let (source, source_is_named) = create_publication_source(&parent, &partial_name)?;
    let source_identity = file_identity(&source)
        .map_err(|_| SqliteOperatorError::Filesystem("envelope source identity"))?;
    let mut file = Some(source);
    let mut published = None;
    let result = (|| {
        let handle = file
            .as_mut()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?;
        handle
            .write_all(bytes)
            .and_then(|()| handle.sync_all())
            .map_err(|_| SqliteOperatorError::Filesystem("sync envelope partial"))?;
        require_exact_open_bytes(handle, source_identity, bytes)?;
        after_sync(&partial)?;
        if source_is_named {
            parent
                .require_relative_identity(&partial_name, source_identity)
                .map_err(|_| SqliteOperatorError::Filesystem("envelope source name changed"))?;
        } else {
            parent
                .require_relative_absent(&partial_name)
                .map_err(|_| SqliteOperatorError::Filesystem("unexpected envelope partial"))?;
        }
        parent
            .require_named()
            .map_err(|_| SqliteOperatorError::Filesystem("envelope parent changed"))?;
        let final_file = match publish_open_file_noreplace(handle, &parent) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(SqliteOperatorError::InvalidArgument(
                    "envelope destination already exists",
                ));
            }
            Err(_) => {
                return Err(SqliteOperatorError::Filesystem(
                    "no-clobber envelope publication",
                ));
            }
        };
        parent
            .sync()
            .map_err(|_| SqliteOperatorError::Filesystem("sync envelope parent"))?;
        require_exact_open_bytes(handle, source_identity, bytes)?;

        let final_identity = file_identity(&final_file)
            .map_err(|_| SqliteOperatorError::Filesystem("published envelope identity"))?;
        parent
            .require_relative_identity(parent.name(), final_identity)
            .map_err(|_| SqliteOperatorError::Filesystem("published envelope identity"))?;
        parent
            .require_named()
            .map_err(|_| SqliteOperatorError::Filesystem("envelope parent changed"))?;
        let mut artifact = RetainedArtifact {
            path: path.to_owned(),
            file: Some(final_file),
            identity: final_identity,
        };
        artifact.require_exact_bytes(bytes)?;
        published = Some(artifact);

        if source_is_named {
            #[cfg(windows)]
            {
                parent.require_relative_absent(&partial_name).map_err(|_| {
                    SqliteOperatorError::Filesystem("renamed envelope source remained named")
                })?;
            }
            #[cfg(not(windows))]
            if !scrub_exact_file(handle, source_identity) {
                return Err(SqliteOperatorError::IncompleteArtifact);
            }
        } else {
            parent
                .require_relative_absent(&partial_name)
                .map_err(|_| SqliteOperatorError::Filesystem("unexpected envelope partial"))?;
        }
        parent
            .sync()
            .map_err(|_| SqliteOperatorError::Filesystem("sync envelope parent"))?;

        published
            .as_mut()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?
            .require_exact_bytes(bytes)?;
        parent
            .require_named()
            .map_err(|_| SqliteOperatorError::Filesystem("envelope parent changed"))?;
        Ok(())
    })();
    if result.is_err() {
        let mut cleanup_complete = true;
        if let Some(artifact) = published.as_mut() {
            cleanup_complete &= artifact.scrub();
        }
        if let Some(handle) = file.as_ref() {
            cleanup_complete &= scrub_exact_file(handle, source_identity);
        }
        cleanup_complete &= parent.sync().is_ok();
        if !cleanup_complete {
            return Err(SqliteOperatorError::IncompleteArtifact);
        }
    }
    result?;
    published.ok_or(SqliteOperatorError::IncompleteArtifact)
}

fn create_publication_source(
    parent: &PublicationParent,
    partial_name: &OsStr,
) -> Result<(File, bool), SqliteOperatorError> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{Mode, OFlags};

        let descriptor = rustix::fs::openat(
            parent.directory(),
            ".",
            OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        );
        return descriptor
            .map(|descriptor| (File::from(descriptor), false))
            .map_err(|_| SqliteOperatorError::Filesystem("create anonymous envelope source"));
    }
    #[cfg(target_os = "macos")]
    {
        use rustix::fs::{Mode, OFlags};

        let descriptor = rustix::fs::openat(
            parent.directory(),
            partial_name,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| SqliteOperatorError::Filesystem("create envelope partial"))?;
        return Ok((File::from(descriptor), true));
    }
    #[cfg(windows)]
    {
        return create_owner_only_renameable_file(&parent.path_for(partial_name))
            .map(|file| (file, true))
            .map_err(|_| SqliteOperatorError::Filesystem("create envelope partial"));
    }
    #[allow(unreachable_code)]
    Err(SqliteOperatorError::Filesystem(
        "unsupported envelope publication platform",
    ))
}

pub(crate) fn publish_open_file_noreplace(
    file: &File,
    parent: &PublicationParent,
) -> io::Result<File> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;

        let descriptor_path = format!("/proc/self/fd/{}", file.as_raw_fd());
        rustix::fs::linkat(
            rustix::fs::CWD,
            descriptor_path.as_str(),
            parent.directory(),
            parent.name(),
            rustix::fs::AtFlags::SYMLINK_FOLLOW,
        )
        .map_err(io::Error::from)?;
        parent.require_named()?;
        return file.try_clone();
    }
    #[cfg(target_os = "macos")]
    {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::fclonefileat(
            file,
            parent.directory(),
            parent.name(),
            rustix::fs::CloneFlags::empty(),
        )
        .map_err(io::Error::from)?;
        let final_descriptor = rustix::fs::openat(
            parent.directory(),
            parent.name(),
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let final_file = File::from(final_descriptor);
        if let Err(error) = parent.require_named() {
            if let Ok(identity) = file_identity(&final_file) {
                let _ = scrub_exact_file(&final_file, identity);
            }
            return Err(error);
        }
        return Ok(final_file);
    }
    #[cfg(windows)]
    {
        worldstream_windows_handle::rename_noreplace_at(file, parent.directory(), parent.name())?;
        return file.try_clone();
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported envelope publication platform",
    ))
}

pub(crate) struct PublicationParent {
    path: PathBuf,
    name: OsString,
    directory: File,
    identity: ArtifactIdentity,
}

impl PublicationParent {
    pub(crate) fn open(destination: &Path) -> io::Result<Self> {
        let path = destination
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        let name = destination
            .file_name()
            .ok_or_else(|| io::Error::other("publication destination name"))?
            .to_owned();
        #[cfg(unix)]
        let directory = {
            use std::os::unix::fs::OpenOptionsExt as _;
            let flags = rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC;
            let flags = i32::try_from(flags.bits())
                .map_err(|_| io::Error::other("publication parent flags"))?;
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&path)?
        };
        #[cfg(windows)]
        let directory = worldstream_windows_handle::open_pinned_directory(&path)?;
        #[cfg(not(any(unix, windows)))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported publication parent platform",
        ));
        if !directory.metadata()?.is_dir() {
            return Err(io::Error::other("publication parent is not a directory"));
        }
        let identity = file_identity(&directory)?;
        let parent = Self {
            path,
            name,
            directory,
            identity,
        };
        parent.require_named()?;
        Ok(parent)
    }

    pub(crate) fn directory(&self) -> &File {
        &self.directory
    }

    pub(crate) fn name(&self) -> &OsStr {
        &self.name
    }

    #[cfg(windows)]
    pub(crate) fn path_for(&self, name: &OsStr) -> PathBuf {
        self.path.join(name)
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        self.directory.sync_all()
    }

    pub(crate) fn require_named(&self) -> io::Result<()> {
        if directory_path_identity(&self.path).ok() != Some(self.identity) {
            return Err(io::Error::other("publication parent identity changed"));
        }
        Ok(())
    }

    pub(crate) fn open_relative_read(&self, name: &OsStr) -> io::Result<File> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            let descriptor = rustix::fs::openat(
                &self.directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            return Ok(File::from(descriptor));
        }
        #[cfg(windows)]
        {
            return File::open(self.path_for(name));
        }
        #[allow(unreachable_code)]
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported relative publication open",
        ))
    }

    pub(crate) fn require_relative_identity(
        &self,
        name: &OsStr,
        expected: ArtifactIdentity,
    ) -> io::Result<()> {
        let file = self.open_relative_read(name)?;
        if file_identity(&file)? != expected {
            return Err(io::Error::other("relative publication identity changed"));
        }
        Ok(())
    }

    pub(crate) fn require_relative_absent(&self, name: &OsStr) -> io::Result<()> {
        match self.open_relative_read(name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "unexpected relative publication entry",
            )),
            Err(error) => Err(error),
        }
    }
}

fn require_exact_open_bytes(
    file: &mut File,
    identity: ArtifactIdentity,
    expected: &[u8],
) -> Result<(), SqliteOperatorError> {
    if file_identity(file).ok() != Some(identity) {
        return Err(SqliteOperatorError::Filesystem(
            "envelope source identity changed",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| SqliteOperatorError::Filesystem("seek envelope source"))?;
    let mut observed = Vec::new();
    Read::by_ref(file)
        .take(
            u64::try_from(expected.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut observed)
        .map_err(|_| SqliteOperatorError::Filesystem("read envelope source"))?;
    if observed != expected || file_identity(file).ok() != Some(identity) {
        return Err(SqliteOperatorError::Filesystem(
            "envelope source bytes changed",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn remove_named_identity_with_hook<F>(
    path: &Path,
    identity: ArtifactIdentity,
    after_identity_check: F,
) -> Result<(), SqliteOperatorError>
where
    F: FnOnce() -> Result<(), SqliteOperatorError>,
{
    if path_identity(path).ok() != Some(identity) {
        return Err(SqliteOperatorError::Filesystem(
            "envelope partial identity changed",
        ));
    }
    after_identity_check()?;
    if path_identity(path).ok() != Some(identity) {
        return Err(SqliteOperatorError::Filesystem(
            "envelope partial identity changed",
        ));
    }
    Err(SqliteOperatorError::Filesystem(
        "pathname cleanup is forbidden",
    ))
}

fn random_nonce() -> Result<String, SqliteOperatorError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SqliteOperatorError::Filesystem("artifact nonce"))?;
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

fn sync_directory(path: &Path) -> Result<(), SqliteOperatorError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| SqliteOperatorError::Filesystem("sync artifact directory"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArtifactFingerprint {
    byte_len: u64,
    digest: [u8; 32],
}

struct RetainedInput {
    path: PathBuf,
    file: File,
    identity: ArtifactIdentity,
}

impl RetainedInput {
    fn open_owner_only(path: &Path) -> Result<Self, SqliteOperatorError> {
        validate_owner_only_file(path)
            .map_err(|_| SqliteOperatorError::Filesystem("owner-only retained input"))?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            options.share_mode(FILE_SHARE_READ);
        }
        let file = options
            .open(path)
            .map_err(|_| SqliteOperatorError::Filesystem("open retained input"))?;
        let identity = file_identity(&file)
            .map_err(|_| SqliteOperatorError::Filesystem("retained input identity"))?;
        let input = Self {
            path: path.to_owned(),
            file,
            identity,
        };
        input.revalidate()?;
        Ok(input)
    }

    fn revalidate(&self) -> Result<(), SqliteOperatorError> {
        validate_owner_only_file(&self.path)
            .map_err(|_| SqliteOperatorError::Filesystem("owner-only retained input"))?;
        if path_identity(&self.path).ok() != Some(self.identity)
            || file_identity(&self.file).ok() != Some(self.identity)
        {
            return Err(SqliteOperatorError::Filesystem(
                "retained input identity changed",
            ));
        }
        Ok(())
    }

    fn canonical_identity(&self) -> (String, String) {
        canonical_artifact_identity(self.identity)
    }

    fn read_bounded(&mut self, limit: usize) -> Result<Vec<u8>, SqliteOperatorError> {
        self.revalidate()?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| SqliteOperatorError::Filesystem("seek retained input"))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut self.file)
            .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| SqliteOperatorError::Filesystem("read retained input"))?;
        if bytes.is_empty() || bytes.len() > limit {
            return Err(SqliteOperatorError::Companion("envelope byte bound"));
        }
        self.revalidate()?;
        Ok(bytes)
    }

    fn fingerprint(&mut self, limit: u64) -> Result<ArtifactFingerprint, SqliteOperatorError> {
        self.revalidate()?;
        let expected_len = self
            .file
            .metadata()
            .map_err(|_| SqliteOperatorError::Filesystem("retained input metadata"))?
            .len();
        if expected_len == 0 || expected_len > limit {
            return Err(SqliteOperatorError::Filesystem("retained input byte bound"));
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| SqliteOperatorError::Filesystem("seek retained input"))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; 1024 * 1024];
        let mut byte_len = 0_u64;
        loop {
            let read = self
                .file
                .read(&mut buffer)
                .map_err(|_| SqliteOperatorError::Filesystem("hash retained input"))?;
            if read == 0 {
                break;
            }
            byte_len = byte_len
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .filter(|value| *value <= limit)
                .ok_or(SqliteOperatorError::Filesystem("retained input byte bound"))?;
            hasher.update(&buffer[..read]);
        }
        if byte_len != expected_len {
            return Err(SqliteOperatorError::Filesystem(
                "retained input length changed",
            ));
        }
        self.revalidate()?;
        Ok(ArtifactFingerprint {
            byte_len,
            digest: *hasher.finalize().as_bytes(),
        })
    }

    fn require_fingerprint(
        &mut self,
        expected: ArtifactFingerprint,
        limit: u64,
    ) -> Result<(), SqliteOperatorError> {
        if self.fingerprint(limit)? != expected {
            return Err(SqliteOperatorError::Filesystem(
                "retained input bytes changed",
            ));
        }
        Ok(())
    }

    fn require_exact_bytes(
        &mut self,
        expected: &[u8],
        limit: usize,
    ) -> Result<(), SqliteOperatorError> {
        if self.read_bounded(limit)? != expected {
            return Err(SqliteOperatorError::Companion(
                "envelope changed during operation",
            ));
        }
        Ok(())
    }

    fn copy_exact_to(
        &mut self,
        destination: &mut File,
        expected: ArtifactFingerprint,
    ) -> Result<(), SqliteOperatorError> {
        self.require_fingerprint(expected, MAX_NATIVE_BACKUP_BYTES)?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| SqliteOperatorError::Filesystem("seek admitted backup"))?;
        if !file_has_single_link(destination) {
            return Err(SqliteOperatorError::Filesystem(
                "admitted snapshot has multiple links",
            ));
        }
        destination
            .set_len(0)
            .and_then(|()| destination.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|_| SqliteOperatorError::Filesystem("prepare admitted snapshot"))?;
        let mut hasher = blake3::Hasher::new();
        let mut byte_len = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = self
                .file
                .read(&mut buffer)
                .map_err(|_| SqliteOperatorError::Filesystem("read admitted backup"))?;
            if read == 0 {
                break;
            }
            byte_len = byte_len
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .filter(|value| *value <= MAX_NATIVE_BACKUP_BYTES)
                .ok_or(SqliteOperatorError::Filesystem(
                    "admitted backup byte bound",
                ))?;
            hasher.update(&buffer[..read]);
            destination
                .write_all(&buffer[..read])
                .map_err(|_| SqliteOperatorError::Filesystem("write admitted snapshot"))?;
        }
        destination
            .sync_all()
            .map_err(|_| SqliteOperatorError::Filesystem("sync admitted snapshot"))?;
        let copied = ArtifactFingerprint {
            byte_len,
            digest: *hasher.finalize().as_bytes(),
        };
        if copied != expected {
            return Err(SqliteOperatorError::Filesystem(
                "admitted backup changed during snapshot",
            ));
        }
        if !file_has_single_link(destination) {
            return Err(SqliteOperatorError::Filesystem(
                "admitted snapshot has multiple links",
            ));
        }
        self.require_fingerprint(expected, MAX_NATIVE_BACKUP_BYTES)
    }
}

struct AdmittedBackupSnapshot {
    directory: PathBuf,
    input: RetainedInput,
    fingerprint: ArtifactFingerprint,
}

impl AdmittedBackupSnapshot {
    fn create(
        source: &mut RetainedInput,
        fingerprint: ArtifactFingerprint,
        parent: &Path,
    ) -> Result<Self, SqliteOperatorError> {
        let directory = parent.join(format!(".worldstream-restore-source-{}", random_nonce()?));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|_| SqliteOperatorError::Filesystem("create admitted snapshot directory"))?;
        let Ok(directory) = prepare_data_directory(&directory) else {
            let _ = fs::remove_dir(&directory);
            return Err(SqliteOperatorError::Filesystem(
                "protect admitted snapshot directory",
            ));
        };
        if sync_directory(parent).is_err() {
            let _ = fs::remove_dir(&directory);
            return Err(SqliteOperatorError::Filesystem(
                "sync admitted snapshot parent",
            ));
        }
        let path = directory.join("admitted.sqlite3");
        let Ok(mut file) = create_owner_only_file(&path) else {
            let _ = fs::remove_dir(&directory);
            return Err(SqliteOperatorError::Filesystem("create admitted snapshot"));
        };
        let copied = source
            .copy_exact_to(&mut file, fingerprint)
            .and_then(|()| sync_directory(&directory));
        if let Err(error) = copied {
            let cleanup = file_identity(&file)
                .ok()
                .is_some_and(|identity| scrub_exact_file(&file, identity));
            if !cleanup {
                return Err(SqliteOperatorError::IncompleteArtifact);
            }
            return Err(error);
        }
        let Ok(identity) = file_identity(&file) else {
            return Err(SqliteOperatorError::IncompleteArtifact);
        };
        let mut snapshot = Self {
            directory,
            input: RetainedInput {
                path,
                file,
                identity,
            },
            fingerprint,
        };
        let validation = snapshot
            .require_fingerprint()
            .and_then(|()| source.require_fingerprint(fingerprint, MAX_NATIVE_BACKUP_BYTES));
        if let Err(error) = validation {
            return match snapshot.scrub_and_remove() {
                Ok(()) => Err(error),
                Err(_) => Err(SqliteOperatorError::IncompleteArtifact),
            };
        }
        Ok(snapshot)
    }

    fn path(&self) -> &Path {
        &self.input.path
    }

    fn require_fingerprint(&mut self) -> Result<(), SqliteOperatorError> {
        self.input
            .require_fingerprint(self.fingerprint, MAX_NATIVE_BACKUP_BYTES)
    }

    fn scrub_and_remove(self) -> Result<(), SqliteOperatorError> {
        self.input.revalidate()?;
        if !scrub_exact_file(&self.input.file, self.input.identity) {
            return Err(SqliteOperatorError::Filesystem("scrub admitted snapshot"));
        }
        self.input.revalidate()?;
        refuse_unadmitted_snapshot_artifacts(
            &self.directory,
            &self.input.path,
            self.input.identity,
        )?;
        sync_directory(&self.directory)
    }
}

fn refuse_unadmitted_snapshot_artifacts(
    directory: &Path,
    admitted_path: &Path,
    admitted_identity: ArtifactIdentity,
) -> Result<(), SqliteOperatorError> {
    let entries = fs::read_dir(directory)
        .map_err(|_| SqliteOperatorError::Filesystem("inventory admitted snapshot directory"))?;
    let mut count = 0_usize;
    for entry in entries {
        count = count.checked_add(1).filter(|value| *value <= 4).ok_or(
            SqliteOperatorError::Filesystem("unexpected admitted snapshot artifacts"),
        )?;
        let entry = entry
            .map_err(|_| SqliteOperatorError::Filesystem("read admitted snapshot artifact"))?;
        if entry.path() != admitted_path
            || path_identity(&entry.path()).ok() != Some(admitted_identity)
            || entry
                .metadata()
                .map_or(true, |metadata| metadata.len() != 0)
        {
            return Err(SqliteOperatorError::Filesystem(
                "unexpected admitted snapshot artifact",
            ));
        }
    }
    if count == 1 {
        Ok(())
    } else {
        Err(SqliteOperatorError::Filesystem(
            "admitted snapshot placeholder missing",
        ))
    }
}

fn revalidate_admitted_backup(
    source: &mut RetainedInput,
    source_fingerprint: ArtifactFingerprint,
    snapshot: &mut AdmittedBackupSnapshot,
) -> Result<(), SqliteOperatorError> {
    source.require_fingerprint(source_fingerprint, MAX_NATIVE_BACKUP_BYTES)?;
    snapshot.require_fingerprint()
}

struct RetainedArtifact {
    path: PathBuf,
    file: Option<File>,
    identity: ArtifactIdentity,
}

impl RetainedArtifact {
    fn open_owner_only(path: &Path) -> Result<Self, SqliteOperatorError> {
        Self::open_owner_only_expected(path, None)
    }

    fn open_owner_only_expected(
        path: &Path,
        expected: Option<NativeSqliteFileIdentityV1>,
    ) -> Result<Self, SqliteOperatorError> {
        let file = open_retained_artifact(path)?;
        let identity = file_identity(&file)
            .map_err(|_| SqliteOperatorError::Filesystem("artifact identity"))?;
        if expected.is_some_and(|expected| !artifact_matches_native(identity, expected)) {
            return Err(SqliteOperatorError::Filesystem(
                "native artifact identity changed",
            ));
        }
        secure_native_artifact(&file, path)?;
        let artifact = Self {
            path: path.to_owned(),
            file: Some(file),
            identity,
        };
        artifact.revalidate()?;
        Ok(artifact)
    }

    fn file(&self) -> Result<&File, SqliteOperatorError> {
        self.file
            .as_ref()
            .ok_or(SqliteOperatorError::IncompleteArtifact)
    }

    fn canonical_identity(&self) -> (String, String) {
        canonical_artifact_identity(self.identity)
    }

    fn revalidate(&self) -> Result<(), SqliteOperatorError> {
        validate_owner_only_file(&self.path)
            .map_err(|_| SqliteOperatorError::Filesystem("owner-only artifact"))?;
        if path_identity(&self.path).ok() != Some(self.identity)
            || self.file.as_ref().and_then(|file| file_identity(file).ok()) != Some(self.identity)
        {
            return Err(SqliteOperatorError::Filesystem("artifact identity changed"));
        }
        Ok(())
    }

    fn require_exact_bytes(&mut self, expected: &[u8]) -> Result<(), SqliteOperatorError> {
        self.revalidate()?;
        let file = self
            .file
            .as_mut()
            .ok_or(SqliteOperatorError::IncompleteArtifact)?;
        let identity = self.identity;
        require_exact_open_bytes(file, identity, expected)?;
        self.revalidate()
    }

    fn scrub(&mut self) -> bool {
        let still_named = self.revalidate().is_ok();
        let scrubbed = self
            .file
            .as_ref()
            .is_some_and(|file| scrub_exact_file(file, self.identity));
        still_named && scrubbed
    }
}

fn secure_native_artifact(file: &File, path: &Path) -> Result<(), SqliteOperatorError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| SqliteOperatorError::Filesystem("protect native artifact"))?;
    }
    validate_owner_only_file(path)
        .map_err(|_| SqliteOperatorError::Filesystem("native artifact permissions"))
}

fn open_retained_artifact(path: &Path) -> Result<File, SqliteOperatorError> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
        options.custom_flags(
            i32::try_from(flags.bits())
                .map_err(|_| SqliteOperatorError::Filesystem("retain native artifact flags"))?,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        options.share_mode(FILE_SHARE_READ);
    }
    options
        .open(path)
        .map_err(|_| SqliteOperatorError::Filesystem("retain native artifact"))
}

#[cfg(unix)]
fn file_identity(file: &File) -> std::io::Result<ArtifactIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
const fn artifact_matches_native(
    identity: ArtifactIdentity,
    expected: NativeSqliteFileIdentityV1,
) -> bool {
    identity.0 == expected.storage_id && identity.1 as u128 == expected.file_id
}

#[cfg(windows)]
const fn artifact_matches_native(
    identity: ArtifactIdentity,
    expected: NativeSqliteFileIdentityV1,
) -> bool {
    identity.storage_id() == expected.storage_id && identity.internal_file_id() == expected.file_id
}

#[cfg(unix)]
fn canonical_artifact_identity(identity: ArtifactIdentity) -> (String, String) {
    (
        format!("{:016x}", identity.0),
        format!("{:032x}", identity.1),
    )
}

#[cfg(windows)]
fn canonical_artifact_identity(identity: ArtifactIdentity) -> (String, String) {
    (
        format!("{:016x}", identity.storage_id()),
        format!("{:032x}", identity.internal_file_id()),
    )
}

#[cfg(unix)]
fn file_has_single_link(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata().is_ok_and(|metadata| metadata.nlink() == 1)
}

#[cfg(windows)]
fn file_has_single_link(file: &File) -> bool {
    worldstream_windows_handle::hard_link_count(file).is_ok_and(|count| count == 1)
}

fn scrub_exact_file(file: &File, expected: ArtifactIdentity) -> bool {
    if file_identity(file).ok() != Some(expected) || !file_has_single_link(file) {
        return false;
    }
    if file.set_len(0).and_then(|()| file.sync_all()).is_err() {
        return false;
    }
    file_identity(file).ok() == Some(expected)
        && file_has_single_link(file)
        && file.metadata().is_ok_and(|metadata| metadata.len() == 0)
}

#[cfg(windows)]
fn file_identity(file: &File) -> std::io::Result<ArtifactIdentity> {
    fs_id::FileID::new(file)
}

#[cfg(unix)]
fn path_identity(path: &Path) -> std::io::Result<ArtifactIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(std::io::Error::other("artifact is not a regular file"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn path_identity(path: &Path) -> std::io::Result<ArtifactIdentity> {
    fs_id::FileID::new(path)
}

#[cfg(unix)]
fn directory_path_identity(path: &Path) -> std::io::Result<ArtifactIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(std::io::Error::other("artifact parent is not a directory"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn directory_path_identity(path: &Path) -> std::io::Result<ArtifactIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(std::io::Error::other("artifact parent is not a directory"));
    }
    fs_id::FileID::new(path)
}

fn finish_or_scrub<T>(
    result: Result<T, SqliteOperatorError>,
    artifacts: &mut [Option<&mut RetainedArtifact>],
) -> Result<T, SqliteOperatorError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let mut complete = true;
            for artifact in artifacts.iter_mut().filter_map(Option::as_deref_mut) {
                complete &= artifact.scrub();
            }
            if complete {
                Err(error)
            } else {
                Err(SqliteOperatorError::IncompleteArtifact)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io::Write as _,
        path::Path,
        sync::Arc,
    };

    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;
    use worldstream_backup::native_sqlite::{
        BUNDLED_SQLITE_VERSION, NativeSqliteLimits, NativeSqliteValueV1, extract_restore_evidence,
    };
    use worldstream_backup::{
        BACKUP_MANIFEST_SCHEMA_V1, BackendNativePointV1, BackendProfileV1, BackupManifestV1,
        DigestV1, MigrationContractV1, MigrationIdentityV1,
        NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1, NativeSqliteAuthoritativeMaterializationV1,
        NativeSqliteBackupEnvelopeV1, NativeSqliteEnvelopeCoverageV1, NativeSqliteEnvelopeOriginV1,
        NativeSqliteNativeWitnessV1, NativeSqliteRequestLedgerV1, NativeSqliteRequestWitnessV1,
        NativeSqliteTimerRelationV1, PackIdentityV1, ResourceBlobV1, ResourceIdentityV1,
        native_evidence_digest, native_membership_digest,
    };
    use worldstream_core::{
        AuthorityBootstrapV1, AuthorityCheckedAt, AuthorityV1, CanonicalJsonV1, CapabilityBearerV1,
        PrincipalKindV1, builtin_counter_registry, counter_v2_digest,
    };
    use worldstream_protocol::{
        AccessMode, BearerWireV1, CreateMember, CreateRoomRequest, PackReference, PrincipalKind,
    };
    use worldstream_runtime::{
        create_owner_only_file, prepare_data_directory, validate_owner_only_file,
    };
    use worldstream_sqlite::SqliteRoomStore;
    use worldstream_transfer::{
        DeploymentIdentityV1, DigestV1 as TransferDigestV1,
        PackIdentityV1 as TransferPackIdentityV1,
    };

    use crate::{GatewayBackend, GatewaySession, sqlite_backend::SqliteGatewayBackend};

    use super::{
        RestoreInputStage, RetainedArtifact, RetainedInput, SqliteOperatorError, backup_sqlite,
        canonical_artifact_identity, decode_strict_json, file_identity, finish_or_scrub,
        publish_owner_only, publish_owner_only_with_hook, remove_named_identity_with_hook,
        restore_sqlite, restore_sqlite_inner, verify_sqlite,
    };

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct NestedProbe {
        value: String,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct StrictProbe {
        nested: NestedProbe,
    }

    #[test]
    fn strict_envelope_decoder_rejects_unknown_nested_fields() {
        let error = decode_strict_json::<StrictProbe>(
            br#"{"nested":{"value":"bound","unknown_security_field":true}}"#,
        );
        assert_eq!(
            error,
            Err(SqliteOperatorError::Companion(
                "noncanonical or unknown JSON fields"
            ))
        );
    }

    #[test]
    fn shipped_backup_and_restore_require_ready_and_preserve_exact_envelope_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let companion = temporary.path().join("companion.json");
        write_envelope(&companion, &empty_companion(&source)?)?;
        let backup = temporary.path().join("backup.sqlite3");
        let envelope = temporary.path().join("backup.envelope.json");
        let restored = temporary.path().join("restored.sqlite3");

        let backup_result = backup_sqlite(&source, &backup, &companion, &envelope)?;
        assert_eq!(backup_result.semantic_verifier, "pass");
        assert_eq!(backup_result.operation, "backup");
        let exact_envelope = fs::read(&envelope)?;
        assert_eq!(backup_result.envelope_bytes, Some(exact_envelope.len()));
        let (backup_storage_id, backup_file_id) =
            canonical_artifact_identity(file_identity(&File::open(&backup)?)?);
        let (envelope_storage_id, envelope_file_id) =
            canonical_artifact_identity(file_identity(&File::open(&envelope)?)?);
        assert_eq!(backup_result.output_storage_id, Some(backup_storage_id));
        assert_eq!(backup_result.output_file_id, Some(backup_file_id));
        assert_eq!(backup_result.envelope_storage_id, Some(envelope_storage_id));
        assert_eq!(backup_result.envelope_file_id, Some(envelope_file_id));
        let restore_result = restore_sqlite(&backup, &envelope, &restored)?;
        assert_eq!(restore_result.semantic_verifier, "pass");
        assert_eq!(restore_result.operation, "restore");
        let (restored_storage_id, restored_file_id) =
            canonical_artifact_identity(file_identity(&File::open(&restored)?)?);
        assert_eq!(restore_result.output_storage_id, Some(restored_storage_id));
        assert_eq!(restore_result.output_file_id, Some(restored_file_id));
        assert_eq!(
            restore_result.envelope_storage_id,
            backup_result.envelope_storage_id
        );
        assert_eq!(
            restore_result.envelope_file_id,
            backup_result.envelope_file_id
        );
        assert_eq!(fs::read(&envelope)?, exact_envelope);
        let native_only = verify_sqlite(&restored)?;
        assert_eq!(native_only.semantic_verifier, "not_invoked");
        assert_no_restore_snapshot_artifacts(temporary.path())?;
        Ok(())
    }

    #[test]
    fn template_resource_and_pack_substitution_fail_before_complete_backup()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let template = empty_companion(&source)?;

        let mut resource_substitution = template.clone();
        resource_substitution.resources[0].bytes[0] ^= 1;
        let companion = temporary.path().join("resource-substitution.json");
        write_envelope(&companion, &resource_substitution)?;
        let backup = temporary.path().join("resource-substitution.sqlite3");
        let envelope = temporary.path().join("resource-substitution.envelope.json");
        assert!(backup_sqlite(&source, &backup, &companion, &envelope).is_err());
        assert_eq!(fs::metadata(&backup)?.len(), 0);
        assert!(!envelope.exists());

        let mut pack_substitution = template;
        pack_substitution.manifest.expected_packs[0].revision_digest =
            DigestV1::hash(b"substituted-pack");
        let companion = temporary.path().join("pack-substitution.json");
        write_envelope(&companion, &pack_substitution)?;
        let backup = temporary.path().join("pack-substitution.sqlite3");
        let envelope = temporary.path().join("pack-substitution.envelope.json");
        assert!(backup_sqlite(&source, &backup, &companion, &envelope).is_err());
        assert_eq!(fs::metadata(&backup)?.len(), 0);
        assert!(!envelope.exists());
        Ok(())
    }

    #[test]
    fn unreferenced_semantic_activation_and_timer_companions_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let template = empty_companion(&source)?;
        for (name, ledger) in [
            ("semantic", NativeSqliteRequestLedgerV1::Semantic),
            ("activation", NativeSqliteRequestLedgerV1::Activation),
        ] {
            let mut substituted = template.clone();
            let request = format!("unreferenced-{name}").into_bytes();
            substituted
                .request_witnesses
                .push(NativeSqliteRequestWitnessV1 {
                    ledger,
                    identity_bytes: format!("operation-{name}").into_bytes(),
                    request_digest: DigestV1::hash(&request),
                    request_bytes: request,
                });
            let companion = temporary.path().join(format!("{name}-extra.json"));
            write_envelope(&companion, &substituted)?;
            let backup = temporary.path().join(format!("{name}-extra.sqlite3"));
            let envelope = temporary.path().join(format!("{name}-extra.envelope.json"));
            assert!(backup_sqlite(&source, &backup, &companion, &envelope).is_err());
            assert_eq!(fs::metadata(&backup)?.len(), 0);
            assert!(!envelope.exists());
        }

        let mut substituted = template;
        substituted
            .timer_relations
            .push(NativeSqliteTimerRelationV1 {
                room_id: "unreferenced-room".to_owned(),
                timer_id: "unreferenced-timer".to_owned(),
                generation: 1,
                fired_transition_seq: Some(1),
            });
        let companion = temporary.path().join("timer-extra.json");
        write_envelope(&companion, &substituted)?;
        let backup = temporary.path().join("timer-extra.sqlite3");
        let envelope = temporary.path().join("timer-extra.envelope.json");
        assert!(backup_sqlite(&source, &backup, &companion, &envelope).is_err());
        assert_eq!(fs::metadata(&backup)?.len(), 0);
        assert!(!envelope.exists());
        Ok(())
    }

    #[test]
    fn missing_request_and_fired_timer_relation_fail_through_public_backup()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        let (template, room_id) = initialize_room_source_and_companion(&source)?;

        let valid_companion = temporary.path().join("valid-companion.json");
        write_envelope(&valid_companion, &template)?;
        let valid_backup = temporary.path().join("valid-backup.sqlite3");
        let valid_envelope = temporary.path().join("valid-backup.envelope.json");
        assert_eq!(
            backup_sqlite(&source, &valid_backup, &valid_companion, &valid_envelope)?
                .semantic_verifier,
            "pass"
        );

        let mut missing_request = template.clone();
        missing_request.request_witnesses.clear();
        let companion = temporary.path().join("missing-request.json");
        write_envelope(&companion, &missing_request)?;
        let backup = temporary.path().join("missing-request.sqlite3");
        let envelope = temporary.path().join("missing-request.envelope.json");
        assert!(backup_sqlite(&source, &backup, &companion, &envelope).is_err());
        assert_eq!(fs::metadata(&backup)?.len(), 0);
        assert!(!envelope.exists());

        let connection = rusqlite::Connection::open(&source)?;
        connection.execute(
            "INSERT INTO timers(room_id, timer_id, generation, scheduled_for, payload_bytes, state) \
             VALUES (?1, 'operator-fired-timer', 1, '2026-08-22T00:00:00Z', ?2, 'fired')",
            rusqlite::params![room_id, b"timer-payload".as_slice()],
        )?;
        drop(connection);
        let companion = temporary.path().join("missing-fired-relation.json");
        write_envelope(&companion, &template)?;
        let backup = temporary.path().join("missing-fired-relation.sqlite3");
        let envelope = temporary
            .path()
            .join("missing-fired-relation.envelope.json");
        assert!(backup_sqlite(&source, &backup, &companion, &envelope).is_err());
        assert_eq!(fs::metadata(&backup)?.len(), 0);
        assert!(!envelope.exists());
        Ok(())
    }

    #[test]
    fn restore_rejects_nonexact_envelope_bytes_before_creating_destination()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let companion = temporary.path().join("companion.json");
        write_envelope(&companion, &empty_companion(&source)?)?;
        let backup = temporary.path().join("backup.sqlite3");
        let envelope = temporary.path().join("backup.envelope.json");
        backup_sqlite(&source, &backup, &companion, &envelope)?;
        let tampered = temporary.path().join("tampered.envelope.json");
        let mut bytes = fs::read(&envelope)?;
        bytes.push(b'\n');
        write_owner_only(&tampered, &bytes)?;
        let restored = temporary.path().join("restored.sqlite3");
        assert!(restore_sqlite(&backup, &tampered, &restored).is_err());
        assert!(!restored.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn retained_input_rejects_same_byte_path_swap() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let path = temporary.path().join("input");
        write_owner_only(&path, b"same bytes")?;
        let retained = RetainedInput::open_owner_only(&path)?;
        fs::rename(&path, temporary.path().join("original"))?;
        write_owner_only(&path, b"same bytes")?;
        assert!(retained.revalidate().is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn restore_consumes_the_admitted_backup_when_caller_path_is_swapped()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;

        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let companion = temporary.path().join("companion.json");
        write_envelope(&companion, &empty_companion(&source)?)?;
        let backup = temporary.path().join("backup.sqlite3");
        let envelope = temporary.path().join("backup.envelope.json");
        backup_sqlite(&source, &backup, &companion, &envelope)?;

        let alternate_source = temporary.path().join("alternate-source.sqlite3");
        initialize_empty_source(&alternate_source)?;
        let connection = rusqlite::Connection::open(&alternate_source)?;
        connection.execute(
            "UPDATE canonical_export_metadata SET deployment_lineage = \
             'deployment/operator-storage-alternate' WHERE metadata_id = 1",
            [],
        )?;
        drop(connection);
        let alternate_companion = temporary.path().join("alternate-companion.json");
        write_envelope(&alternate_companion, &empty_companion(&alternate_source)?)?;
        let alternate_backup = temporary.path().join("alternate-backup.sqlite3");
        let alternate_envelope = temporary.path().join("alternate-backup.envelope.json");
        backup_sqlite(
            &alternate_source,
            &alternate_backup,
            &alternate_companion,
            &alternate_envelope,
        )?;

        let admitted_name = temporary.path().join("admitted-backup.sqlite3");
        let restored = temporary.path().join("restored.sqlite3");
        let result = restore_sqlite_inner(
            &backup,
            &envelope,
            &restored,
            |stage, _admitted_path| -> Result<(), SqliteOperatorError> {
                match stage {
                    RestoreInputStage::BeforeNativeRestore => {
                        fs::rename(&backup, &admitted_name).map_err(|_| {
                            SqliteOperatorError::Filesystem("test retain admitted pathname")
                        })?;
                        fs::rename(&alternate_backup, &backup).map_err(|_| {
                            SqliteOperatorError::Filesystem("test install alternate pathname")
                        })?;
                    }
                    RestoreInputStage::AfterNativeRestore => {
                        fs::rename(&backup, &alternate_backup).map_err(|_| {
                            SqliteOperatorError::Filesystem("test remove alternate pathname")
                        })?;
                        fs::rename(&admitted_name, &backup).map_err(|_| {
                            SqliteOperatorError::Filesystem("test restore admitted pathname")
                        })?;
                    }
                    RestoreInputStage::BeforeSnapshotCleanup => {}
                }
                Ok(())
            },
        )?;
        assert_eq!(result.semantic_verifier, "pass");
        assert_eq!(
            extract_restore_evidence(&restored, NativeSqliteLimits::default())?
                .deployment_lineage
                .as_deref(),
            Some("deployment/operator-storage-test")
        );
        Ok(())
    }

    #[test]
    fn verifier_failure_scrubs_destination_and_removes_admitted_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let companion = temporary.path().join("companion.json");
        write_envelope(&companion, &empty_companion(&source)?)?;
        let backup = temporary.path().join("backup.sqlite3");
        let envelope = temporary.path().join("backup.envelope.json");
        backup_sqlite(&source, &backup, &companion, &envelope)?;

        let alternate_source = temporary.path().join("alternate-source.sqlite3");
        initialize_empty_source(&alternate_source)?;
        let connection = rusqlite::Connection::open(&alternate_source)?;
        connection.execute(
            "UPDATE canonical_export_metadata SET deployment_lineage = \
             'deployment/operator-storage-verifier-failure' WHERE metadata_id = 1",
            [],
        )?;
        drop(connection);
        let alternate_companion = temporary.path().join("alternate-companion.json");
        write_envelope(&alternate_companion, &empty_companion(&alternate_source)?)?;
        let alternate_backup = temporary.path().join("alternate-backup.sqlite3");
        let alternate_envelope = temporary.path().join("alternate-backup.envelope.json");
        backup_sqlite(
            &alternate_source,
            &alternate_backup,
            &alternate_companion,
            &alternate_envelope,
        )?;

        let restored = temporary.path().join("restored.sqlite3");
        assert!(restore_sqlite(&backup, &alternate_envelope, &restored).is_err());
        assert_eq!(fs::metadata(&restored)?.len(), 0);
        assert_no_restore_snapshot_artifacts(temporary.path())?;
        Ok(())
    }

    #[test]
    fn snapshot_cleanup_failure_cannot_report_ready_and_scrubs_destination()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let source = temporary.path().join("source.sqlite3");
        initialize_empty_source(&source)?;
        let companion = temporary.path().join("companion.json");
        write_envelope(&companion, &empty_companion(&source)?)?;
        let backup = temporary.path().join("backup.sqlite3");
        let envelope = temporary.path().join("backup.envelope.json");
        backup_sqlite(&source, &backup, &companion, &envelope)?;
        let restored = temporary.path().join("restored.sqlite3");

        let result = restore_sqlite_inner(&backup, &envelope, &restored, |stage, admitted_path| {
            if stage == RestoreInputStage::BeforeSnapshotCleanup {
                write_owner_only(
                    &admitted_path
                        .parent()
                        .ok_or(SqliteOperatorError::Filesystem("test snapshot parent"))?
                        .join("unexpected-artifact"),
                    b"cleanup fault",
                )
                .map_err(|_| SqliteOperatorError::Filesystem("test cleanup fault"))?;
            }
            Ok(())
        });
        assert_eq!(result, Err(SqliteOperatorError::IncompleteArtifact));
        assert_eq!(fs::metadata(&restored)?.len(), 0);
        assert!(fs::read_dir(temporary.path())?.any(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().to_str().map(str::to_owned))
                .is_some_and(|name| name.starts_with(".worldstream-restore-source-"))
        }));
        Ok(())
    }

    #[test]
    fn envelope_publication_is_no_clobber_and_failure_scrubs_every_retained_artifact()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let destination = temporary.path().join("existing-envelope");
        write_owner_only(&destination, b"existing")?;
        assert!(publish_owner_only(&destination, b"replacement").is_err());
        assert_eq!(fs::read(&destination)?, b"existing");

        let first = temporary.path().join("first-artifact");
        let second = temporary.path().join("second-artifact");
        write_owner_only(&first, b"native-backup")?;
        write_owner_only(&second, b"published-envelope")?;
        let mut first = RetainedArtifact::open_owner_only(&first)?;
        let mut second = RetainedArtifact::open_owner_only(&second)?;
        let result: Result<(), SqliteOperatorError> = finish_or_scrub(
            Err(SqliteOperatorError::VerificationRejected),
            &mut [Some(&mut first), Some(&mut second)],
        );
        assert_eq!(result, Err(SqliteOperatorError::VerificationRejected));
        assert_eq!(
            first
                .file
                .as_ref()
                .map(|file| file.metadata().map(|value| value.len()))
                .transpose()?,
            Some(0)
        );
        assert_eq!(fs::metadata(&first.path)?.len(), 0);
        assert_eq!(fs::metadata(&second.path)?.len(), 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn envelope_publication_substitution_never_leaves_replacement_bytes_at_final()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let destination = temporary.path().join("backup.envelope.json");
        let held_original = temporary.path().join("held-original.partial");

        let result = publish_owner_only_with_hook(&destination, b"admitted envelope", |partial| {
            if partial.exists() {
                fs::rename(partial, &held_original)
                    .map_err(|_| SqliteOperatorError::Filesystem("test partial substitution"))?;
            }
            let mut replacement = create_owner_only_file(partial)
                .map_err(|_| SqliteOperatorError::Filesystem("test partial substitution"))?;
            replacement
                .write_all(b"substituted envelope")
                .and_then(|()| replacement.sync_all())
                .map_err(|_| SqliteOperatorError::Filesystem("test partial substitution"))
        });

        assert!(result.is_err());
        assert!(
            !destination.exists() || fs::metadata(&destination)?.len() == 0,
            "failed publication left attacker-controlled bytes at the final path"
        );
        if held_original.exists() {
            assert_eq!(fs::metadata(&held_original)?.len(), 0);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn envelope_publication_rejects_destination_parent_substitution()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let admitted_parent = temporary.path().join("admitted");
        let held_parent = temporary.path().join("held-admitted");
        fs::create_dir(&admitted_parent)?;
        prepare_test_directory(&admitted_parent)?;
        let destination = admitted_parent.join("backup.envelope.json");

        let result = publish_owner_only_with_hook(&destination, b"admitted envelope", |_| {
            fs::rename(&admitted_parent, &held_parent)
                .map_err(|_| SqliteOperatorError::Filesystem("test parent substitution"))?;
            fs::create_dir(&admitted_parent)
                .map_err(|_| SqliteOperatorError::Filesystem("test parent substitution"))?;
            prepare_test_directory(&admitted_parent)
                .map_err(|_| SqliteOperatorError::Filesystem("test parent substitution"))?;
            Ok(())
        });

        assert!(result.is_err());
        assert!(
            !destination.exists() || fs::metadata(&destination)?.len() == 0,
            "publication reached the replacement parent"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn envelope_cleanup_never_unlinks_a_substituted_name() -> Result<(), Box<dyn std::error::Error>>
    {
        let temporary = tempdir()?;
        prepare_test_directory(temporary.path())?;
        let partial = temporary.path().join("partial");
        let held = temporary.path().join("held-original");
        let replacement = b"same-owner replacement";
        let mut original = create_owner_only_file(&partial)?;
        original.write_all(b"admitted")?;
        original.sync_all()?;
        let identity = file_identity(&original)?;

        let result = remove_named_identity_with_hook(&partial, identity, || {
            fs::rename(&partial, &held)
                .map_err(|_| SqliteOperatorError::Filesystem("test cleanup substitution"))?;
            let mut installed = create_owner_only_file(&partial)
                .map_err(|_| SqliteOperatorError::Filesystem("test cleanup substitution"))?;
            installed
                .write_all(replacement)
                .and_then(|()| installed.sync_all())
                .map_err(|_| SqliteOperatorError::Filesystem("test cleanup substitution"))
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&partial)?, replacement);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn initialize_room_source_and_companion(
        path: &Path,
    ) -> Result<(NativeSqliteBackupEnvelopeV1, String), Box<dyn std::error::Error>> {
        initialize_empty_source(path)?;
        let store = SqliteRoomStore::open(path)?;
        let authority = AuthorityV1::new(Arc::new(store.clone()));
        let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse::<worldstream_core::PrincipalId>()?;
        let bootstrap = AuthorityBootstrapV1::new(
            "01ARZ3NDEKTSV4RRFFQ69G5FC4".parse()?,
            principal.clone(),
            PrincipalKindV1::Human,
            "01ARZ3NDEKTSV4RRFFQ69G5FC3".parse()?,
            bearer.token_hash(),
            None,
        )?;
        authority.bootstrap(
            bootstrap,
            "2026-08-15T12:00:00Z".parse::<AuthorityCheckedAt>()?,
        )?;
        let registry = Arc::new(builtin_counter_registry()?);
        let descriptor = registry.load_retained(&counter_v2_digest())?.descriptor();
        let request = CreateRoomRequest {
            pack: PackReference {
                id: descriptor.pack_id.clone(),
                version: descriptor.explanatory_version.clone(),
                digest: counter_v2_digest().to_string(),
            },
            configuration: serde_json::json!({"initial_value": 0, "maximum_value": 16}),
            members: vec![CreateMember {
                principal_id: principal.to_string(),
                principal_kind: PrincipalKind::Human,
                role: Some("counter".to_owned()),
                access_mode: AccessMode::Participant,
            }],
            idempotency_key: "operator-storage-create".to_owned(),
        };
        let backend = SqliteGatewayBackend::new(store.clone(), Arc::clone(&registry));
        let wire = BearerWireV1::from_bytes([0xa9; 32]);
        let session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FC5".parse()?,
            CapabilityBearerV1::from_bytes(BearerWireV1::parse(&wire.to_wire())?.into_bytes()),
            wire,
        );
        let response = backend.create_room(&session, request)?;
        drop(backend);
        drop(authority);
        drop(store);
        normalize_offline_test_source(path)?;

        let evidence = extract_restore_evidence(path, NativeSqliteLimits::default())?;
        let semantic_rows = evidence
            .operational
            .tables
            .get("semantic_receipts")
            .ok_or("semantic receipt table")?;
        let row = semantic_rows.first().ok_or("creation receipt")?;
        let identity = match row.values.get(2) {
            Some(NativeSqliteValueV1::Blob(value)) => value.clone(),
            _ => return Err("creation receipt identity".into()),
        };
        let stored_request_digest = match row.values.get(4) {
            Some(NativeSqliteValueV1::Blob(value)) if value.len() == 32 => value.as_slice(),
            _ => return Err("creation request digest".into()),
        };
        let request_bytes = canonical_creation_request_bytes(principal.as_ref())?;
        if blake3::hash(&request_bytes).as_bytes() != stored_request_digest {
            return Err("creation request bytes".into());
        }

        let mut envelope = empty_companion(path)?;
        envelope
            .request_witnesses
            .push(NativeSqliteRequestWitnessV1 {
                ledger: NativeSqliteRequestLedgerV1::Semantic,
                identity_bytes: identity,
                request_digest: DigestV1::hash(&request_bytes),
                request_bytes,
            });
        for (room_id, head) in &evidence.room_heads {
            let bytes = authoritative_bytes(head)?;
            let digest = DigestV1::hash(&bytes);
            if digest != head.authoritative_state_digest {
                return Err("authoritative materialization".into());
            }
            envelope.authoritative_materializations.push(
                NativeSqliteAuthoritativeMaterializationV1 {
                    room_id: room_id.clone(),
                    source_bytes: bytes.clone(),
                    restored_bytes: bytes,
                    source_digest: digest.clone(),
                    restored_digest: digest,
                },
            );
        }
        Ok((envelope, response.room_id))
    }

    fn canonical_creation_request_bytes(
        principal_id: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "domain": "worldstream/create-room-request/v1",
            "pack_digest": counter_v2_digest().to_string(),
            "configuration": {"initial_value": 0, "maximum_value": 16},
            "ordered_initial_memberships": [{
                "principal_id": principal_id,
                "principal_kind": "human",
                "standing": "enabled",
                "access_mode": "participant",
                "role": "counter"
            }]
        });
        Ok(CanonicalJsonV1::parse(&serde_json::to_vec(&value)?)?.to_bytes()?)
    }

    fn authoritative_bytes(
        head: &worldstream_backup::native_sqlite::NativeSqliteRoomHeadV1,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "domain": "worldstream/authoritative-state/v1",
            "core_schema": head.core_schema_version,
            "pack_digest": format!("blake3:{}", head.pack_digest.as_str()),
            "core_state_hash": format!("blake3:{}", head.core_state_digest.as_str()),
            "activity_state_hash": format!("blake3:{}", head.activity_state_digest.as_str()),
        });
        Ok(CanonicalJsonV1::parse(&serde_json::to_vec(&value)?)?.to_bytes()?)
    }

    fn initialize_empty_source(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteRoomStore::open(path)?;
        store.initialize_canonical_metadata("deployment/operator-storage-test", 3)?;
        let digest = counter_v2_digest();
        let pack = TransferPackIdentityV1::new(
            "worldstream.counter",
            "2.0.0",
            TransferDigestV1::from_bytes(digest.digest().as_bytes())?,
        )?;
        store.initialize_deployment_identity(DeploymentIdentityV1::new(vec![pack], vec![])?)?;
        drop(store);
        normalize_offline_test_source(path)?;
        validate_owner_only_file(path)?;
        Ok(())
    }

    fn normalize_offline_test_source(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let connection = rusqlite::Connection::open(path)?;
        let journal_mode: String =
            connection.query_row("PRAGMA journal_mode=DELETE", (), |row| row.get(0))?;
        if journal_mode != "delete" {
            return Err("offline operator fixture did not enter DELETE journal mode".into());
        }
        drop(connection);
        let header = fs::read(path)?;
        if header.get(18..20) != Some(&[1, 1]) {
            return Err("offline operator fixture is not a standalone SQLite image".into());
        }
        for suffix in ["-journal", "-wal", "-shm"] {
            if std::path::PathBuf::from(format!("{}{}", path.display(), suffix)).exists() {
                return Err(format!("offline operator fixture retained {suffix}").into());
            }
        }
        Ok(())
    }

    fn prepare_test_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        prepare_data_directory(path)?;
        Ok(())
    }

    fn assert_no_restore_snapshot_artifacts(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("snapshot artifact name")?;
            if name.starts_with(".worldstream-restore-source-") {
                let children = fs::read_dir(entry.path())?.collect::<Result<Vec<_>, _>>()?;
                if children.len() != 1
                    || children[0].file_name() != "admitted.sqlite3"
                    || children[0].metadata()?.len() != 0
                {
                    return Err(format!("invalid restore snapshot placeholder: {name}").into());
                }
            } else if name.starts_with("admitted.sqlite3-") {
                return Err(format!("unexpected restore snapshot artifact: {name}").into());
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn empty_companion(
        source_path: &Path,
    ) -> Result<NativeSqliteBackupEnvelopeV1, Box<dyn std::error::Error>> {
        let source = extract_restore_evidence(source_path, NativeSqliteLimits::default())?;
        let migrations = source
            .migration_metadata
            .as_ref()
            .ok_or("migration metadata")?
            .iter()
            .map(
                |row| -> Result<MigrationIdentityV1, Box<dyn std::error::Error>> {
                    let [
                        NativeSqliteValueV1::Integer(version),
                        NativeSqliteValueV1::Text(id),
                        NativeSqliteValueV1::Text(checksum),
                    ] = row.values.as_slice()
                    else {
                        return Err("migration row".into());
                    };
                    Ok(MigrationIdentityV1 {
                        version: u32::try_from(*version)?,
                        migration_id: id.clone(),
                        checksum: DigestV1::parse(
                            checksum
                                .strip_prefix("blake3:")
                                .unwrap_or(checksum)
                                .to_owned(),
                        )?,
                    })
                },
            )
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let executor = trusted_counter_v2_executor_bytes();
        let executor_digest = DigestV1::hash(&executor);
        let native_point = BackendNativePointV1::SqliteOnlineBackup {
            engine_identity: BUNDLED_SQLITE_VERSION.to_owned(),
            point_id: "template-capture-placeholder".to_owned(),
        };
        let lineage = source
            .deployment_lineage
            .clone()
            .ok_or("deployment lineage")?;
        let epoch = source.storage_epoch.ok_or("storage epoch")?;
        let evidence_digest = native_evidence_digest(&source)?;
        let membership_digest = native_membership_digest(&source.operational)?;
        let witness = NativeSqliteNativeWitnessV1 {
            backend: BackendProfileV1::SqliteBundled,
            native_point: native_point.clone(),
            deployment_lineage: lineage.clone(),
            storage_epoch: epoch,
            evidence_digest: evidence_digest.clone(),
            membership_digest: membership_digest.clone(),
        };
        Ok(NativeSqliteBackupEnvelopeV1 {
            schema: NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1.to_owned(),
            origin: NativeSqliteEnvelopeOriginV1 {
                producer: "template-placeholder".to_owned(),
                capture_id: "template-capture-placeholder".to_owned(),
                coverage: "template companion facts".to_owned(),
            },
            manifest: BackupManifestV1 {
                schema: BACKUP_MANIFEST_SCHEMA_V1.to_owned(),
                backup_id: "operator-storage-test".to_owned(),
                deployment_lineage: lineage,
                storage_epoch: epoch,
                backend: BackendProfileV1::SqliteBundled,
                native_point,
                migration_contract: MigrationContractV1 {
                    logical_history_id: "worldstream-storage-v1".to_owned(),
                    schema_contract_fingerprint: DigestV1::parse(
                        "16de6f848ff61583a6b0ad49c0aeeb15e0c6e8a696e21bbe61f41e2d06ad7fcb"
                            .to_owned(),
                    )?,
                    records: migrations,
                },
                expected_packs: vec![PackIdentityV1 {
                    pack_id: "worldstream.counter".to_owned(),
                    revision_digest: DigestV1::parse(
                        "1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92"
                            .to_owned(),
                    )?,
                    executor_digest: executor_digest.clone(),
                    schema_bundle_digest: DigestV1::parse(
                        "0d4f81253a26d5a4ac85ae461e37426df905c555ea2071b9834a631d4b2e99ee"
                            .to_owned(),
                    )?,
                    codec_bundle_digest: DigestV1::parse(
                        "67b814baf1511b6c29ffe9862eeeb2b130988972c9c3a2c2ab6ff1f7018a6b30"
                            .to_owned(),
                    )?,
                    resource_ids: vec!["fixture-executor".to_owned()],
                }],
                expected_resources: vec![ResourceIdentityV1 {
                    resource_id: "fixture-executor".to_owned(),
                    kind: "executor".to_owned(),
                    byte_len: u64::try_from(executor.len())?,
                    digest: executor_digest,
                }],
                expected_global_digest: DigestV1::hash(&[]),
            },
            resources: vec![ResourceBlobV1 {
                resource_id: "fixture-executor".to_owned(),
                bytes: executor,
            }],
            request_witnesses: Vec::new(),
            authoritative_materializations: Vec::new(),
            timer_relations: Vec::new(),
            source: witness.clone(),
            restored: witness,
            coverage: NativeSqliteEnvelopeCoverageV1 {
                tables: required_coverage_tables(),
                source_evidence_digest: evidence_digest.clone(),
                restored_evidence_digest: evidence_digest,
                source_membership_digest: membership_digest.clone(),
                restored_membership_digest: membership_digest,
            },
            envelope_digest: DigestV1::hash(&[]),
        })
    }

    fn required_coverage_tables() -> Vec<String> {
        [
            "retired_authority_fences_v1",
            "principals",
            "runners",
            "capabilities",
            "capability_scopes",
            "runner_capability_memberships",
            "authority_change_receipts",
            "authority_audit",
            "room_integrity",
            "room_members",
            "timers",
            "observation_frames",
            "observation_consequences",
            "activation_decisions",
            "activation_intents",
            "activation_operation_receipts",
            "semantic_receipts",
            "integrity_incidents",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn trusted_counter_v2_executor_bytes() -> Vec<u8> {
        let source = include_bytes!("../../worldstream-core/src/counter.rs");
        let mut artifact = b"worldstream/counter-executor-source/v1\0".to_vec();
        artifact.extend_from_slice(b"2.0.0");
        artifact.push(0);
        let mut index = 0;
        while index < source.len() {
            if source[index] == b'\r' {
                artifact.push(b'\n');
                if source.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            } else {
                artifact.push(source[index]);
            }
            index += 1;
        }
        artifact
    }

    fn write_envelope(
        path: &Path,
        envelope: &NativeSqliteBackupEnvelopeV1,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_owner_only(path, &serde_json::to_vec(envelope)?)
    }

    fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = create_owner_only_file(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        validate_owner_only_file(path)?;
        Ok(())
    }
}
