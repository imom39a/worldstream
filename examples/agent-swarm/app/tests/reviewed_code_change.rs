use std::{env, fs};

use worldstream_agent_swarm::{
    ArtifactPath,
    checks::{
        CheckCommand, CheckOutcome, CheckRunner, EvidenceBinding, ReviewError, ReviewEvidence,
        ReviewPolicy, ReviewVerdict, ReviewerKind, VersionedInput,
    },
    code_change::CodeChangeWorkspace,
};

struct ReviewedFixture {
    _temporary: tempfile::TempDir,
    changes: CodeChangeWorkspace,
    draft: worldstream_agent_swarm::code_change::CodeCandidateDraft,
    revision: worldstream_agent_swarm::code_change::CodeCandidateRevision,
    binding: EvidenceBinding,
}

fn fixture() -> Result<ReviewedFixture, Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let working = temporary.path().join("working");
    fs::create_dir(&working)?;
    fs::write(working.join("answer.txt"), b"old answer")?;
    let changes = CodeChangeWorkspace::open(&working, &temporary.path().join("protected"))?;
    let path = ArtifactPath::new("answer.txt")?;
    let draft = changes.prepare_candidate("reviewed-candidate", std::slice::from_ref(&path))?;
    fs::write(draft.path(&path)?, b"reviewed answer")?;
    let revision = changes.seal_candidate(&draft)?;
    let baseline = draft.targets()[0]
        .artifact()
        .cloned()
        .ok_or("missing fixture baseline")?;
    let criteria = changes
        .artifacts()
        .store_bytes(b"answer.txt contains the reviewed answer")?;
    let binding = EvidenceBinding::new(
        &revision,
        vec![VersionedInput {
            name: "answer-baseline".to_owned(),
            artifact: baseline,
        }],
        criteria,
    )?;
    Ok(ReviewedFixture {
        _temporary: temporary,
        changes,
        draft,
        revision,
        binding,
    })
}

fn checker_command(fail: bool) -> Result<CheckCommand, Box<dyn std::error::Error>> {
    let command = CheckCommand::new(
        "deterministic-check",
        env::current_exe()?,
        vec![
            "--exact".to_owned(),
            "controlled_checker_fixture".to_owned(),
            "--nocapture".to_owned(),
        ],
    )?;
    if fail {
        Ok(command.with_environment("WORLDSTREAM_CONTROLLED_CHECK_FAIL", "1")?)
    } else {
        Ok(command)
    }
}

#[test]
fn controlled_checker_fixture() {
    assert!(
        env::var_os("WORLDSTREAM_CONTROLLED_CHECK_FAIL").is_none(),
        "controlled checker failure"
    );
}

#[test]
fn exact_check_and_independent_review_accept_the_revision() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = fixture()?;
    let evidence = CheckRunner::new(&fixture.changes).run(
        &fixture.revision,
        &fixture.binding,
        &checker_command(false)?,
    )?;
    assert_eq!(evidence.outcome, CheckOutcome::Passed);
    let review = ReviewEvidence::new(
        "review-1",
        "agent-reviewer",
        ReviewerKind::Agent,
        fixture.binding.clone(),
        ReviewVerdict::Pass,
        Vec::new(),
    )?;
    let accepted = ReviewPolicy::new(vec!["deterministic-check".to_owned()])?.accept(
        &fixture.changes,
        &fixture.revision,
        &fixture.binding,
        &["agent-author".to_owned()],
        &[evidence],
        &[review],
    )?;
    assert_eq!(
        accepted.candidate_digest,
        fixture.revision.candidate_digest()
    );
    assert_eq!(accepted.independent_reviewers, ["agent-reviewer"]);
    Ok(())
}

#[test]
fn failed_or_stale_check_evidence_cannot_accept() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture()?;
    let failed = CheckRunner::new(&fixture.changes).run(
        &fixture.revision,
        &fixture.binding,
        &checker_command(true)?,
    )?;
    assert!(matches!(failed.outcome, CheckOutcome::Failed { .. }));
    let review = ReviewEvidence::new(
        "review-2",
        "agent-reviewer",
        ReviewerKind::Agent,
        fixture.binding.clone(),
        ReviewVerdict::Pass,
        Vec::new(),
    )?;
    let policy = ReviewPolicy::new(vec!["deterministic-check".to_owned()])?;
    assert!(matches!(
        policy.accept(
            &fixture.changes,
            &fixture.revision,
            &fixture.binding,
            &["agent-author".to_owned()],
            std::slice::from_ref(&failed),
            std::slice::from_ref(&review),
        ),
        Err(ReviewError::FailedCheck)
    ));

    let path = ArtifactPath::new("answer.txt")?;
    fs::write(fixture.draft.path(&path)?, b"different candidate")?;
    let newer = fixture.changes.seal_candidate(&fixture.draft)?;
    assert!(matches!(
        policy.accept(
            &fixture.changes,
            &newer,
            &fixture.binding,
            &["agent-author".to_owned()],
            &[failed],
            &[review],
        ),
        Err(ReviewError::StaleEvidence)
    ));
    Ok(())
}

#[test]
fn self_review_and_unresolved_findings_remain_blocking() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture()?;
    let check = CheckRunner::new(&fixture.changes).run(
        &fixture.revision,
        &fixture.binding,
        &checker_command(false)?,
    )?;
    let policy = ReviewPolicy::new(vec!["deterministic-check".to_owned()])?;
    let self_review = ReviewEvidence::new(
        "review-self",
        "agent-author",
        ReviewerKind::Agent,
        fixture.binding.clone(),
        ReviewVerdict::Pass,
        Vec::new(),
    )?;
    assert!(matches!(
        policy.accept(
            &fixture.changes,
            &fixture.revision,
            &fixture.binding,
            &["agent-author".to_owned()],
            std::slice::from_ref(&check),
            &[self_review],
        ),
        Err(ReviewError::IndependentReviewMissing)
    ));

    let blocking = ReviewEvidence::new(
        "review-block",
        "agent-reviewer-a",
        ReviewerKind::Agent,
        fixture.binding.clone(),
        ReviewVerdict::Block,
        vec!["the result is incomplete".to_owned()],
    )?;
    let later_pass = ReviewEvidence::new(
        "review-pass",
        "agent-reviewer-b",
        ReviewerKind::Agent,
        fixture.binding.clone(),
        ReviewVerdict::Pass,
        Vec::new(),
    )?;
    assert!(matches!(
        policy.accept(
            &fixture.changes,
            &fixture.revision,
            &fixture.binding,
            &["agent-author".to_owned()],
            &[check],
            &[blocking, later_pass],
        ),
        Err(ReviewError::BlockingReview)
    ));
    Ok(())
}
