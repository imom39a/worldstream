#![cfg(feature = "managed-local-runtime")]

use std::{collections::BTreeMap, env, fs, path::Path};

use worldstream_agent_swarm::{
    ArtifactError, ArtifactPath, CodeChangeService, CodeChangeServiceError, ContentDigest,
    PackArtifactRef, PackCandidateRef, PackVersionRef, PrepareCodeChangeRequest,
    RecordCodeReviewRequest, ReviewGate, RunCodeCheckRequest, SealCodeChangeRequest,
    StageCodeWriteBackRequest,
    checks::{ReviewVerdict, ReviewerKind},
    code_change::WriteBackDisposition,
};
use worldstream_core::CanonicalJsonV1;

const GUARD: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-process-guard");

#[test]
fn controlled_code_change_checker() {}

fn checker_request(
    candidate_id: &str,
    revision_digest: ContentDigest,
    check_id: &str,
    evidence_path: &str,
    pack_check_revision: u64,
) -> Result<RunCodeCheckRequest, Box<dyn std::error::Error>> {
    Ok(RunCodeCheckRequest {
        arguments: vec![
            "--exact".to_owned(),
            "controlled_code_change_checker".to_owned(),
            "--nocapture".to_owned(),
        ],
        candidate_id: candidate_id.to_owned(),
        check_id: check_id.to_owned(),
        evidence_path: ArtifactPath::new(evidence_path)?,
        environment: BTreeMap::new(),
        pack_check_revision,
        program: env::current_exe()?.canonicalize()?,
        revision_digest,
    })
}

fn seal_candidate(
    service: &CodeChangeService,
    candidate_id: &str,
    desired: &[u8],
    criteria: Vec<String>,
) -> Result<worldstream_agent_swarm::SealedCodeChange, Box<dyn std::error::Error>> {
    let path = ArtifactPath::new("answer.txt")?;
    let prepared = service.prepare(&PrepareCodeChangeRequest {
        candidate_id: candidate_id.to_owned(),
        targets: vec![path.clone()],
    })?;
    fs::write(prepared.editable_root.join(path.as_str()), desired)?;
    Ok(service.seal(&SealCodeChangeRequest {
        acceptance_criteria: criteria,
        authors: vec!["author-a".to_owned()],
        candidate_id: candidate_id.to_owned(),
        criteria_revision: 7,
        inputs: Vec::new(),
        pack_candidate: PackCandidateRef {
            candidate_id: candidate_id.to_owned(),
            version: 3,
        },
        pack_candidate_artifact: None,
        pack_candidate_path: None,
        resource_basis: vec![PackVersionRef {
            resource_id: "goal-a".to_owned(),
            version: 5,
        }],
    })?)
}

fn accept_revision(
    service: &CodeChangeService,
    candidate_id: &str,
    revision_digest: ContentDigest,
    evidence_prefix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let check = service.run_check(
        &checker_request(
            candidate_id,
            revision_digest,
            "criterion-1",
            &format!("evidence/{evidence_prefix}-criterion-1.json"),
            11,
        )?,
        Path::new(GUARD),
    )?;
    assert_eq!(check.pack_reference, "check:criterion-1:11");
    let review = service.record_review(&RecordCodeReviewRequest {
        candidate_id: candidate_id.to_owned(),
        findings: Vec::new(),
        pack_review_revision: 13,
        review_id: format!("review-{candidate_id}"),
        reviewer_id: "reviewer-a".to_owned(),
        reviewer_kind: ReviewerKind::Agent,
        revision_digest,
        verdict: ReviewVerdict::Pass,
    })?;
    assert_eq!(review.gate, ReviewGate::Accepted);
    Ok(())
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end test keeps the exact persistence and recovery story contiguous"
)]
fn durable_workflow_binds_exact_evidence_and_recovers_idempotent_writeback()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let working = temporary.path().join("working");
    let protected = temporary.path().join("protected");
    fs::create_dir(&working)?;
    fs::create_dir(working.join("evidence"))?;
    fs::write(working.join("answer.txt"), b"baseline")?;
    let service = CodeChangeService::open(&working, &protected)?;
    let candidate_id = "candidate-durable";
    let sealed = seal_candidate(
        &service,
        candidate_id,
        b"reviewed answer",
        vec!["first criterion".to_owned(), "second criterion".to_owned()],
    )?;
    assert_eq!(sealed.required_checks, ["criterion-1", "criterion-2"]);

    // Execute out of order; the accepted references must still follow the
    // acceptance-criteria order fixed at sealing time.
    let second = service.run_check(
        &checker_request(
            candidate_id,
            sealed.revision_digest,
            "criterion-2",
            "evidence/durable-criterion-2.json",
            22,
        )?,
        Path::new(GUARD),
    )?;
    assert_eq!(second.pack_reference, "check:criterion-2:22");
    let first_request = checker_request(
        candidate_id,
        sealed.revision_digest,
        "criterion-1",
        "evidence/durable-criterion-1.json",
        21,
    )?;
    let first = service.run_check(&first_request, Path::new(GUARD))?;
    assert_eq!(first.pack_reference, "check:criterion-1:21");
    assert_eq!(first.action_payload.evidence_refs.len(), 1);
    let evidence_ref = &first.action_payload.evidence_refs[0];
    assert_eq!(evidence_ref.candidate, sealed.pack_candidate);
    assert_eq!(evidence_ref.candidate_artifact, sealed.candidate_artifact);
    assert_eq!(evidence_ref.check_id, "criterion-1");
    assert_eq!(evidence_ref.criterion, "first criterion");
    assert_eq!(evidence_ref.criteria_revision, 7);
    assert_eq!(evidence_ref.resource_basis[0].resource_id, "goal-a");
    assert_eq!(
        evidence_ref.artifact.media_type,
        "application/vnd.worldstream.agent-swarm-check-evidence+json;version=1"
    );
    assert_eq!(
        evidence_ref.artifact.local_path,
        working
            .canonicalize()?
            .join("evidence/durable-criterion-1.json")
            .display()
            .to_string()
    );
    let evidence_bytes = fs::read(&evidence_ref.artifact.local_path)?;
    let canonical = CanonicalJsonV1::parse(&evidence_bytes)?.to_bytes()?;
    assert_eq!(canonical, evidence_bytes);
    assert_eq!(
        ContentDigest::of(&evidence_bytes).to_string(),
        evidence_ref.artifact.digest
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&evidence_bytes)?,
        serde_json::to_value(&first.evidence)?
    );
    let evidence_object = serde_json::to_value(evidence_ref)?;
    assert_eq!(
        evidence_object
            .as_object()
            .ok_or("evidence ref was not an object")?
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "artifact",
            "candidate",
            "candidate_artifact",
            "check_id",
            "criteria_revision",
            "criterion",
            "resource_basis"
        ]
    );
    let artifact_object = evidence_object["artifact"]
        .as_object()
        .ok_or("artifact ref was not an object")?;
    assert_eq!(
        artifact_object
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["artifact_id", "digest", "local_path", "media_type"]
    );
    // Model a crash after the pending outcome became durable but before the
    // authorized evidence and final journal record were both observable. The
    // exact retry recovers retained evidence and cannot execute the checker
    // again (the editable candidate no longer matches the revision).
    let draft = service.workspace().open_candidate(candidate_id)?;
    fs::write(draft.root().join("answer.txt"), b"later candidate")?;
    remove_read_only_file(Path::new(&evidence_ref.artifact.local_path))?;
    let final_check = protected
        .join("reviewed-code-change/candidates")
        .join(candidate_id)
        .join(sealed.revision_digest.hex())
        .join("checks/criterion-1.json");
    remove_read_only_file(&final_check)?;
    assert_eq!(service.run_check(&first_request, Path::new(GUARD))?, first);

    let review = service.record_review(&RecordCodeReviewRequest {
        candidate_id: candidate_id.to_owned(),
        findings: Vec::new(),
        pack_review_revision: 31,
        review_id: "review-durable".to_owned(),
        reviewer_id: "reviewer-a".to_owned(),
        reviewer_kind: ReviewerKind::Agent,
        revision_digest: sealed.revision_digest,
        verdict: ReviewVerdict::Pass,
    })?;
    assert_eq!(review.gate, ReviewGate::Accepted);
    assert_eq!(
        review.check_references,
        ["check:criterion-1:21", "check:criterion-2:22"]
    );
    assert_eq!(
        review
            .accepted
            .as_ref()
            .ok_or("revision was not accepted")?
            .check_ids,
        ["criterion-1", "criterion-2"]
    );

    // A later edit and seal cannot mutate the retained revision or its
    // content-addressed bytes.
    let later = service.workspace().seal_candidate(&draft)?;
    assert_ne!(later.revision_digest(), sealed.revision_digest);
    let reopened = service
        .workspace()
        .open_candidate_revision(candidate_id, sealed.revision_digest)?;
    let reviewed_artifact = reopened.targets()[0]
        .candidate()
        .ok_or("sealed candidate artifact missing")?;
    assert_eq!(
        service.workspace().artifacts().read(reviewed_artifact)?,
        b"reviewed answer"
    );

    let operation = service.stage_write_back(&StageCodeWriteBackRequest {
        candidate_id: candidate_id.to_owned(),
        operation_id: "write-durable".to_owned(),
        revision_digest: sealed.revision_digest,
    })?;
    drop(service);

    let reopened_service = CodeChangeService::open(&working, &protected)?;
    let status = reopened_service.status()?;
    assert_eq!(status.candidates.len(), 1);
    assert_eq!(status.candidates[0].revisions.len(), 2);
    assert_eq!(status.write_backs.len(), 1);
    assert_eq!(status.write_backs[0].operation, operation);
    assert!(status.write_backs[0].status.is_none());
    let applied = reopened_service.reconcile_write_back("write-durable")?;
    assert_eq!(applied.disposition, WriteBackDisposition::Applied);
    assert_eq!(fs::read(working.join("answer.txt"))?, b"reviewed answer");

    fs::write(working.join("answer.txt"), b"later human edit")?;
    assert_eq!(
        reopened_service.reconcile_write_back("write-durable")?,
        applied
    );
    assert_eq!(fs::read(working.join("answer.txt"))?, b"later human edit");
    Ok(())
}

fn remove_read_only_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn writeback_detects_drift_and_preserves_newer_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let working = temporary.path().join("working");
    let protected = temporary.path().join("protected");
    fs::create_dir(&working)?;
    fs::create_dir(working.join("evidence"))?;
    fs::write(working.join("answer.txt"), b"baseline")?;
    let service = CodeChangeService::open(&working, &protected)?;
    let candidate_id = "candidate-conflict";
    let sealed = seal_candidate(
        &service,
        candidate_id,
        b"candidate",
        vec!["criterion".to_owned()],
    )?;
    accept_revision(&service, candidate_id, sealed.revision_digest, "conflict")?;
    service.stage_write_back(&StageCodeWriteBackRequest {
        candidate_id: candidate_id.to_owned(),
        operation_id: "write-conflict".to_owned(),
        revision_digest: sealed.revision_digest,
    })?;
    fs::write(working.join("answer.txt"), b"newer human bytes")?;

    let blocked = service.reconcile_write_back("write-conflict")?;
    assert_eq!(blocked.disposition, WriteBackDisposition::BlockedConflict);
    assert_eq!(fs::read(working.join("answer.txt"))?, b"newer human bytes");
    assert_eq!(service.reconcile_write_back("write-conflict")?, blocked);
    Ok(())
}

#[test]
fn supplied_room_candidate_artifact_is_verified_and_bound_into_check_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let working = temporary.path().join("working");
    let protected = temporary.path().join("protected");
    fs::create_dir(&working)?;
    fs::create_dir(working.join("evidence"))?;
    let candidate_bytes = b"supplied material result";
    fs::write(working.join("report.txt"), candidate_bytes)?;
    let service = CodeChangeService::open(&working, &protected)?;
    let target = ArtifactPath::new("report.txt")?;
    service.prepare(&PrepareCodeChangeRequest {
        candidate_id: "report-candidate".to_owned(),
        targets: vec![target.clone()],
    })?;
    let room_artifact = PackArtifactRef {
        artifact_id: "room-report-artifact".to_owned(),
        digest: ContentDigest::of(candidate_bytes).to_string(),
        local_path: working
            .canonicalize()?
            .join("report.txt")
            .display()
            .to_string(),
        media_type: "text/plain".to_owned(),
    };
    let sealed = service.seal(&SealCodeChangeRequest {
        acceptance_criteria: vec!["The exact supplied report passes.".to_owned()],
        authors: vec!["author-a".to_owned()],
        candidate_id: "report-candidate".to_owned(),
        criteria_revision: 1,
        inputs: Vec::new(),
        pack_candidate: PackCandidateRef {
            candidate_id: "report-candidate".to_owned(),
            version: 1,
        },
        pack_candidate_artifact: Some(room_artifact.clone()),
        pack_candidate_path: Some(target),
        resource_basis: Vec::new(),
    })?;
    assert_eq!(sealed.candidate_artifact, room_artifact);
    let checked = service.run_check(
        &checker_request(
            "report-candidate",
            sealed.revision_digest,
            "criterion-1",
            "evidence/report-criterion-1.json",
            1,
        )?,
        Path::new(GUARD),
    )?;
    let reference = &checked.action_payload.evidence_refs[0];
    assert_eq!(reference.candidate_artifact, room_artifact);
    assert_eq!(reference.criterion, "The exact supplied report passes.");

    service.prepare(&PrepareCodeChangeRequest {
        candidate_id: "bad-candidate".to_owned(),
        targets: vec![ArtifactPath::new("report.txt")?],
    })?;
    let Err(error) = service.seal(&SealCodeChangeRequest {
        acceptance_criteria: vec!["The exact supplied report passes.".to_owned()],
        authors: vec!["author-a".to_owned()],
        candidate_id: "bad-candidate".to_owned(),
        criteria_revision: 1,
        inputs: Vec::new(),
        pack_candidate: PackCandidateRef {
            candidate_id: "bad-candidate".to_owned(),
            version: 1,
        },
        pack_candidate_artifact: Some(PackArtifactRef {
            artifact_id: "room-report-artifact".to_owned(),
            digest: format!("blake3:{}", "0".repeat(64)),
            local_path: working
                .canonicalize()?
                .join("report.txt")
                .display()
                .to_string(),
            media_type: "text/plain".to_owned(),
        }),
        pack_candidate_path: Some(ArtifactPath::new("report.txt")?),
        resource_basis: Vec::new(),
    }) else {
        return Err("mismatched Room artifact digest was accepted".into());
    };
    assert!(matches!(
        error,
        CodeChangeServiceError::Artifact(ArtifactError::DigestMismatch)
    ));
    Ok(())
}
