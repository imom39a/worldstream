use std::fs;

use serde_json::json;
use worldstream_agent_swarm::{
    ArtifactError, ArtifactPath, ArtifactWorkspace, AuthoritativeArtifactRef, ContentDigest,
    authoritative_artifact_for_path,
};

fn workspace() -> Result<(tempfile::TempDir, ArtifactWorkspace), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let working = temporary.path().join("working");
    fs::create_dir(&working)?;
    let artifacts = ArtifactWorkspace::open(&working, &temporary.path().join("protected"))?;
    Ok((temporary, artifacts))
}

#[test]
fn capture_is_content_addressed_and_immutable() -> Result<(), Box<dyn std::error::Error>> {
    let (temporary, artifacts) = workspace()?;
    let source = temporary.path().join("working/source.txt");
    fs::write(&source, b"first version")?;
    let path = ArtifactPath::new("source.txt")?;

    let first = artifacts.capture(&path)?;
    let duplicate = artifacts.store_bytes(b"first version")?;
    assert_eq!(first, duplicate);
    assert_eq!(artifacts.read(&first)?, b"first version");

    fs::write(source, b"second version")?;
    let second = artifacts.capture(&path)?;
    assert_ne!(first.digest(), second.digest());
    assert_eq!(artifacts.read(&first)?, b"first version");
    assert_eq!(artifacts.read(&second)?, b"second version");

    let encoded = serde_json::to_string(&first.digest())?;
    let decoded: ContentDigest = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, first.digest());
    Ok(())
}

#[test]
fn portable_paths_cannot_escape_the_authorized_root() {
    for invalid in [
        "",
        "../outside",
        "a/../outside",
        "/absolute",
        "a\\windows",
        "C:/drive",
        "nested//file",
        "CON.txt",
        "trailing.",
    ] {
        assert_eq!(
            ArtifactPath::new(invalid).err(),
            Some(ArtifactError::InvalidPath),
            "unexpectedly accepted {invalid:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn capture_rejects_symlink_files_and_directories() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let (temporary, artifacts) = workspace()?;
    let working = temporary.path().join("working");
    let outside = temporary.path().join("outside");
    fs::create_dir(&outside)?;
    fs::write(outside.join("secret.txt"), b"not authorized")?;
    symlink(outside.join("secret.txt"), working.join("file-link"))?;
    symlink(&outside, working.join("directory-link"))?;

    assert_eq!(
        artifacts.capture(&ArtifactPath::new("file-link")?).err(),
        Some(ArtifactError::RedirectedPath)
    );
    assert_eq!(
        artifacts
            .capture(&ArtifactPath::new("directory-link/secret.txt")?)
            .err(),
        Some(ArtifactError::RedirectedPath)
    );
    Ok(())
}

#[test]
fn only_the_final_component_may_be_absent() -> Result<(), Box<dyn std::error::Error>> {
    let (_temporary, artifacts) = workspace()?;
    assert_eq!(
        artifacts.capture_if_present(&ArtifactPath::new("new.txt")?)?,
        None
    );
    assert_eq!(
        artifacts
            .capture_if_present(&ArtifactPath::new("missing/new.txt")?)
            .err(),
        Some(ArtifactError::ArtifactMissing)
    );
    Ok(())
}

#[test]
fn authoritative_room_reference_is_required_and_repeated_exact_refs_are_deduplicated()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, artifacts) = workspace()?;
    let path = ArtifactPath::new("source.txt")?;
    fs::write(
        temporary.path().join("working/source.txt"),
        b"room-authorized",
    )?;
    let captured = artifacts.capture(&path)?;
    let local_path = artifacts
        .authorized_root()
        .join(path.as_str())
        .display()
        .to_string();
    let reference = json!({
        "artifact_id": "artifact-source",
        "digest": captured.digest().to_string(),
        "local_path": local_path,
        "media_type": "text/plain"
    });
    let activity = json!({
        "contributions": [{"artifact": reference.clone()}],
        "candidates": [{"artifacts": [reference.clone()]}],
        "results": [{"artifact": reference}]
    });

    let authoritative =
        authoritative_artifact_for_path(&activity, artifacts.authorized_root(), &path)?;
    assert_eq!(authoritative.artifact_id, "artifact-source");
    assert_eq!(
        artifacts.capture_authoritative(&path, &authoritative)?,
        captured
    );

    let missing = ArtifactPath::new("missing.txt")?;
    assert_eq!(
        authoritative_artifact_for_path(&activity, artifacts.authorized_root(), &missing).err(),
        Some(ArtifactError::UnreferencedArtifact)
    );
    Ok(())
}

#[test]
fn conflicting_room_refs_and_digest_mismatches_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, artifacts) = workspace()?;
    let path = ArtifactPath::new("source.txt")?;
    fs::write(
        temporary.path().join("working/source.txt"),
        b"authoritative",
    )?;
    let local_path = artifacts
        .authorized_root()
        .join(path.as_str())
        .display()
        .to_string();
    let first = json!({
        "artifact_id": "artifact-source",
        "digest": ContentDigest::of(b"authoritative").to_string(),
        "local_path": local_path,
        "media_type": "text/plain"
    });
    let conflicting = json!({
        "artifact_id": "artifact-source",
        "digest": ContentDigest::of(b"other").to_string(),
        "local_path": first["local_path"],
        "media_type": "text/plain"
    });
    assert_eq!(
        authoritative_artifact_for_path(
            &json!([first, conflicting]),
            artifacts.authorized_root(),
            &path
        )
        .err(),
        Some(ArtifactError::AmbiguousArtifact)
    );

    let bad_digest = json!({
        "artifact_id": "artifact-source",
        "digest": ContentDigest::of(b"other").to_string(),
        "local_path": artifacts.authorized_root().join(path.as_str()),
        "media_type": "text/plain"
    });
    let authoritative =
        authoritative_artifact_for_path(&bad_digest, artifacts.authorized_root(), &path)?;
    assert_eq!(
        artifacts.capture_authoritative(&path, &authoritative).err(),
        Some(ArtifactError::DigestMismatch)
    );
    Ok(())
}

#[test]
fn bounded_authoritative_capture_re_resolves_room_authority_before_reading()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, artifacts) = workspace()?;
    let bytes = b"patch\n";
    fs::write(temporary.path().join("working/change.diff"), bytes)?;
    let selected = AuthoritativeArtifactRef {
        artifact_id: "artifact-change".to_owned(),
        digest: ContentDigest::of(bytes).to_string(),
        local_path: artifacts
            .authorized_root()
            .join("change.diff")
            .display()
            .to_string(),
        media_type: "text/x-diff".to_owned(),
    };
    let activity = serde_json::to_value(json!({"candidates":[{"artifact":selected}]}))?;
    let selected: AuthoritativeArtifactRef =
        serde_json::from_value(activity["candidates"][0]["artifact"].clone())?;

    let (path, captured) = artifacts.capture_authoritative_bounded(&activity, &selected, 64)?;
    assert_eq!(path.as_str(), "change.diff");
    assert_eq!(artifacts.read(&captured)?, bytes);
    assert_eq!(
        artifacts
            .capture_authoritative_bounded(&activity, &selected, 5)
            .err(),
        Some(ArtifactError::ArtifactTooLarge)
    );

    let mut rebound = selected.clone();
    rebound.artifact_id = "different-id".to_owned();
    assert_eq!(
        artifacts
            .capture_authoritative_bounded(&activity, &rebound, 64)
            .err(),
        Some(ArtifactError::InvalidReference)
    );
    Ok(())
}

#[test]
fn selected_content_aliases_preserve_identity_and_reject_conflicting_claims()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, artifacts) = workspace()?;
    let source = temporary.path().join("working/shared.txt");
    fs::write(&source, b"same source")?;
    let contribution = AuthoritativeArtifactRef {
        artifact_id: "contribution".to_owned(),
        digest: ContentDigest::of(b"same source").to_string(),
        local_path: artifacts
            .authorized_root()
            .join("shared.txt")
            .display()
            .to_string(),
        media_type: "text/plain".to_owned(),
    };
    let candidate = AuthoritativeArtifactRef {
        artifact_id: "candidate".to_owned(),
        ..contribution.clone()
    };
    let activity =
        json!({"contributions":[{"artifact":contribution}],"candidates":[{"artifact":candidate}]});
    for selected in [&contribution, &candidate] {
        let (_, captured) = artifacts.capture_authoritative_bounded(&activity, selected, 64)?;
        assert_eq!(artifacts.read(&captured)?, b"same source");
    }
    // A pathname alone still cannot choose which semantic identity was intended.
    assert_eq!(
        authoritative_artifact_for_path(
            &activity,
            artifacts.authorized_root(),
            &ArtifactPath::new("shared.txt")?
        )
        .err(),
        Some(ArtifactError::AmbiguousArtifact)
    );
    let unknown = AuthoritativeArtifactRef {
        artifact_id: "absent".to_owned(),
        ..candidate.clone()
    };
    assert_eq!(
        artifacts
            .capture_authoritative_bounded(&activity, &unknown, 64)
            .err(),
        Some(ArtifactError::InvalidReference)
    );
    for (field, value) in [
        ("digest", ContentDigest::of(b"different").to_string()),
        ("media_type", "text/x-python".to_owned()),
    ] {
        let mut conflicting = activity.clone();
        conflicting["contributions"][0]["artifact"][field] = json!(value);
        assert_eq!(
            artifacts
                .capture_authoritative_bounded(&conflicting, &candidate, 64)
                .err(),
            Some(ArtifactError::AmbiguousArtifact)
        );
    }
    fs::write(source, b"changed")?;
    assert_eq!(
        artifacts
            .capture_authoritative_bounded(&activity, &candidate, 64)
            .err(),
        Some(ArtifactError::DigestMismatch)
    );
    Ok(())
}

#[test]
fn generated_evidence_publication_is_immutable_and_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let (temporary, artifacts) = workspace()?;
    fs::create_dir(temporary.path().join("working/evidence"))?;
    let path = ArtifactPath::new("evidence/check.json")?;
    let first = artifacts.publish_generated(&path, br#"{"passed":true}"#)?;
    let repeated = artifacts.publish_generated(&path, br#"{"passed":true}"#)?;
    assert_eq!(first, repeated);
    assert_eq!(
        artifacts
            .publish_generated(&path, br#"{"passed":false}"#)
            .err(),
        Some(ArtifactError::ChangedDuringCapture)
    );
    assert_eq!(artifacts.read(&first)?, br#"{"passed":true}"#);
    Ok(())
}
