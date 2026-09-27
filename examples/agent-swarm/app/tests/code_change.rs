use std::fs;

use worldstream_agent_swarm::{
    ArtifactPath,
    code_change::{
        CodeChangeError, CodeChangeWorkspace, WriteBackDisposition, WriteBackTargetOutcome,
    },
};

fn workspace() -> Result<(tempfile::TempDir, CodeChangeWorkspace), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::create_dir(temporary.path().join("working"))?;
    let changes = CodeChangeWorkspace::open(
        &temporary.path().join("working"),
        &temporary.path().join("protected"),
    )?;
    Ok((temporary, changes))
}

#[test]
fn candidate_edits_are_isolated_until_a_sealed_revision_is_applied()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, changes) = workspace()?;
    let original = temporary.path().join("working/example.txt");
    fs::write(&original, b"baseline\n")?;
    let path = ArtifactPath::new("example.txt")?;

    let draft = changes.prepare_candidate("candidate-a", std::slice::from_ref(&path))?;
    fs::write(draft.path(&path)?, b"candidate\n")?;
    assert_eq!(fs::read(&original)?, b"baseline\n");

    let revision = changes.seal_candidate(&draft)?;
    let operation = changes.stage_write_back("write-a", &revision)?;
    assert_eq!(fs::read(&original)?, b"baseline\n");
    let applied = changes.reconcile_write_back(&operation)?;
    assert!(applied.succeeded());
    assert_eq!(fs::read(&original)?, b"candidate\n");

    fs::write(&original, b"later human edit\n")?;
    let replayed = changes.reconcile_write_back(&operation)?;
    assert_eq!(
        replayed, applied,
        "terminal operation record must be stable"
    );
    assert_eq!(fs::read(&original)?, b"later human edit\n");
    Ok(())
}

#[test]
fn concurrent_edits_are_preserved_as_content_addressed_conflicts()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, changes) = workspace()?;
    let original = temporary.path().join("working/example.txt");
    fs::write(&original, b"baseline")?;
    let path = ArtifactPath::new("example.txt")?;
    let draft = changes.prepare_candidate("candidate-b", std::slice::from_ref(&path))?;
    fs::write(draft.path(&path)?, b"candidate")?;
    let revision = changes.seal_candidate(&draft)?;
    let operation = changes.stage_write_back("write-b", &revision)?;

    fs::write(&original, b"newer human bytes")?;
    let status = changes.reconcile_write_back(&operation)?;
    assert_eq!(status.disposition, WriteBackDisposition::BlockedConflict);
    assert_eq!(fs::read(&original)?, b"newer human bytes");
    let WriteBackTargetOutcome::Conflict {
        current: Some(current),
    } = &status.targets[0].outcome
    else {
        return Err("expected a retained conflict artifact".into());
    };
    assert_eq!(changes.artifacts().read(current)?, b"newer human bytes");
    assert_eq!(changes.reconcile_write_back(&operation)?, status);
    Ok(())
}

#[test]
fn reconciliation_recognizes_success_after_a_lost_reply() -> Result<(), Box<dyn std::error::Error>>
{
    let (temporary, changes) = workspace()?;
    let original = temporary.path().join("working/example.txt");
    fs::write(&original, b"baseline")?;
    let path = ArtifactPath::new("example.txt")?;
    let draft = changes.prepare_candidate("candidate-c", std::slice::from_ref(&path))?;
    fs::write(draft.path(&path)?, b"candidate")?;
    let revision = changes.seal_candidate(&draft)?;
    let operation = changes.stage_write_back("write-c", &revision)?;

    // Models a process that changed the target, then crashed before persisting
    // or acknowledging its operation result.
    fs::write(&original, b"candidate")?;
    let recovered = changes.reconcile_write_back(&operation)?;
    assert!(recovered.succeeded());
    assert!(matches!(
        recovered.targets[0].outcome,
        WriteBackTargetOutcome::Applied { .. }
    ));
    Ok(())
}

#[test]
fn stable_operation_identity_cannot_be_rebound() -> Result<(), Box<dyn std::error::Error>> {
    let (temporary, changes) = workspace()?;
    fs::write(temporary.path().join("working/example.txt"), b"baseline")?;
    let path = ArtifactPath::new("example.txt")?;
    let draft = changes.prepare_candidate("candidate-d", std::slice::from_ref(&path))?;
    fs::write(draft.path(&path)?, b"first candidate")?;
    let first = changes.seal_candidate(&draft)?;
    changes.stage_write_back("stable-write", &first)?;

    fs::write(draft.path(&path)?, b"second candidate")?;
    let second = changes.seal_candidate(&draft)?;
    assert_eq!(
        changes.stage_write_back("stable-write", &second).err(),
        Some(CodeChangeError::OperationIdentityConflict)
    );
    Ok(())
}

#[test]
fn new_files_and_deletions_use_the_same_versioned_operation()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, changes) = workspace()?;
    let removed = ArtifactPath::new("removed.txt")?;
    let added = ArtifactPath::new("added.txt")?;
    fs::write(temporary.path().join("working/removed.txt"), b"remove me")?;
    let draft = changes.prepare_candidate("candidate-e", &[removed.clone(), added.clone()])?;
    fs::remove_file(draft.path(&removed)?)?;
    fs::write(draft.path(&added)?, b"new file")?;
    let revision = changes.seal_candidate(&draft)?;
    let operation = changes.stage_write_back("write-e", &revision)?;
    let status = changes.reconcile_write_back(&operation)?;

    assert!(status.succeeded());
    assert!(!temporary.path().join("working/removed.txt").exists());
    assert_eq!(
        fs::read(temporary.path().join("working/added.txt"))?,
        b"new file"
    );
    Ok(())
}
