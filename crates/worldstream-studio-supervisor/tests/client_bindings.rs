use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;
use worldstream_activity_client::ExactPackReferenceV1;
use worldstream_protocol::AccessMode;
use worldstream_studio_supervisor::client_bindings::{
    ClientBindingStoreV1, ClientCandidateClassV1, ClientSelectionRequestV1, ClientSelectionV1,
};
use worldstream_studio_supervisor::initialization_inputs::parse_client_declaration;

const PACK_DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RELEASE_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ARTIFACT_DIGEST: &str =
    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn release(client_id: &str, surface_id: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "worldstream/activity-client-release/v1",
        "client_id": client_id,
        "release_digest": RELEASE_DIGEST,
        "client_contract": "worldstream/activity-client-protocol/v1",
        "artifacts": [{
            "artifact_id": "browser-dist",
            "media_type": "application/vnd.worldstream.activity-client.web.v1+tar",
            "digest": ARTIFACT_DIGEST
        }],
        "surfaces": [
            {
                "surface_id": surface_id,
                "kind": "browser",
                "artifact_digest": ARTIFACT_DIGEST,
                "entrypoint": "/heist/",
                "capabilities": ["observe", "act"]
            },
            {
                "surface_id": "inspector-web",
                "kind": "browser",
                "artifact_digest": ARTIFACT_DIGEST,
                "entrypoint": "/inspector/",
                "capabilities": ["observe", "act", "replay"]
            }
        ],
        "conformance": []
    }))
    .unwrap_or_else(|error| unreachable!("release fixture: {error}"))
}

fn bootstrap(second_candidate: bool) -> Vec<u8> {
    let mut deployments = vec![
        json!({
            "schema": "worldstream/client-deployment/v1",
            "deployment_id": "heist-primary",
            "client_id": "example.heist.web",
            "release_digest": RELEASE_DIGEST,
            "trust_level": "verified",
            "verification_evidence_digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "surfaces": [{
                "surface_id": "participant-web",
                "launch_url": "http://127.0.0.1:5173/heist/"
            }]
        }),
        json!({
            "schema": "worldstream/client-deployment/v1",
            "deployment_id": "inspector-host",
            "client_id": "example.heist.web",
            "release_digest": RELEASE_DIGEST,
            "trust_level": "verified",
            "verification_evidence_digest": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "surfaces": [{
                "surface_id": "inspector-web",
                "launch_url": "http://127.0.0.1:5173/inspector/"
            }]
        }),
    ];
    let mut bindings = vec![json!({
        "schema": "worldstream/client-binding/v1",
        "binding_id": "heist-primary-navigator",
        "pack": { "id": "example.heist", "version": "1.0.0", "digest": PACK_DIGEST },
        "client_contract": "worldstream/activity-client-protocol/v1",
        "access_mode": "participant",
        "roles": ["navigator"],
        "deployment_id": "heist-primary",
        "surface_id": "participant-web",
        "preference": "eligible"
    })];
    if second_candidate {
        deployments.push(json!({
            "schema": "worldstream/client-deployment/v1",
            "deployment_id": "heist-secondary",
            "client_id": "example.heist.web",
            "release_digest": RELEASE_DIGEST,
            "trust_level": "externally_trusted",
            "surfaces": [{
                "surface_id": "participant-web",
                "launch_url": "https://clients.worldstream.example/heist/"
            }]
        }));
        bindings.push(json!({
            "schema": "worldstream/client-binding/v1",
            "binding_id": "heist-secondary-navigator",
            "pack": { "id": "example.heist", "version": "1.0.0", "digest": PACK_DIGEST },
            "client_contract": "worldstream/activity-client-protocol/v1",
            "access_mode": "participant",
            "roles": ["navigator"],
            "deployment_id": "heist-secondary",
            "surface_id": "participant-web",
            "preference": "eligible"
        }));
    }
    serde_json::to_vec(&json!({
        "schema": "worldstream/client-binding-bootstrap/v1",
        "deployment_trust_policy": "allow_externally_trusted",
        "deployments": deployments,
        "bindings": bindings,
        "inspector_fallback": {
            "schema": "worldstream/inspector-fallback/v1",
            "fallback_id": "configured-inspector",
            "deployment_id": "inspector-host",
            "surface_id": "inspector-web"
        }
    }))
    .unwrap_or_else(|error| unreachable!("bootstrap fixture: {error}"))
}

fn request(role: Option<&str>) -> ClientSelectionRequestV1 {
    ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: "example.heist".to_owned(),
            version: "1.0.0".to_owned(),
            digest: PACK_DIGEST.to_owned(),
        },
        client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
        access_mode: AccessMode::Participant,
        role: role.map(str::to_owned),
    }
}

#[test]
fn selects_only_an_exact_current_membership_binding_and_survives_restart() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let root = state.path().join("client-bindings");
    let release = release("example.heist.web", "participant-web");
    let store = ClientBindingStoreV1::open(&root, &[release.as_slice()], &bootstrap(false))
        .unwrap_or_else(|error| unreachable!("store: {error}"));

    let selected = store
        .select(&request(Some("navigator")), None)
        .unwrap_or_else(|error| unreachable!("selection: {error}"));
    let ClientSelectionV1::Selected { candidate } = selected else {
        unreachable!("the exact binding must select")
    };
    assert_eq!(candidate.deployment_id, "heist-primary");
    assert_eq!(candidate.launch_url, "http://127.0.0.1:5173/heist/");

    assert!(matches!(
        store.select(&request(Some("broker")), None),
        Ok(ClientSelectionV1::InspectorFallback { candidate })
            if candidate.launch_url == "http://127.0.0.1:5173/inspector/"
    ));
    let restarted = ClientBindingStoreV1::open(&root, &[], &bootstrap(false))
        .unwrap_or_else(|error| unreachable!("restart: {error}"));
    assert!(matches!(
        restarted.select(&request(Some("navigator")), None),
        Ok(ClientSelectionV1::Selected { .. })
    ));
}

#[test]
fn requires_an_explicit_approved_choice_for_multiple_viable_clients() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let root = state.path().join("client-bindings");
    let release = release("example.heist.web", "participant-web");
    let store = ClientBindingStoreV1::open(&root, &[release.as_slice()], &bootstrap(true))
        .unwrap_or_else(|error| unreachable!("store: {error}"));

    let selection = store
        .select(&request(Some("navigator")), None)
        .unwrap_or_else(|error| unreachable!("selection: {error}"));
    let ClientSelectionV1::SelectionRequired { candidates } = selection else {
        unreachable!("two eligible clients require a choice")
    };
    assert_eq!(candidates.len(), 2);
    assert!(matches!(
        store.select(
            &request(Some("navigator")),
            Some("heist-secondary-navigator"),
        ),
        Ok(ClientSelectionV1::Selected { candidate }) if candidate.deployment_id == "heist-secondary"
    ));
}

#[test]
fn host_trust_policy_filters_externally_trusted_deployments() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let root = state.path().join("client-bindings");
    let release = release("example.heist.web", "participant-web");
    let mut bootstrap: serde_json::Value = serde_json::from_slice(&bootstrap(true))
        .unwrap_or_else(|error| unreachable!("bootstrap fixture: {error}"));
    bootstrap["deployment_trust_policy"] = json!("verified_only");
    let bootstrap = serde_json::to_vec(&bootstrap)
        .unwrap_or_else(|error| unreachable!("bootstrap bytes: {error}"));
    let store = ClientBindingStoreV1::open(&root, &[release.as_slice()], &bootstrap)
        .unwrap_or_else(|error| unreachable!("store: {error}"));

    assert!(matches!(
        store.select(&request(Some("navigator")), None),
        Ok(ClientSelectionV1::Selected { candidate })
            if candidate.deployment_id == "heist-primary"
    ));
    assert!(matches!(
        store.select(
            &request(Some("navigator")),
            Some("heist-secondary-navigator"),
        ),
        Err(worldstream_studio_supervisor::client_bindings::ClientBindingStoreErrorV1::InvalidChoice)
    ));
}

#[test]
fn externally_trusted_deployments_require_exact_safe_https_urls() {
    let release = release("example.heist.web", "participant-web");
    for invalid in [
        "http://clients.worldstream.example/heist/",
        "https://localhost/heist/",
        "https://127.0.0.1/heist/",
        "https://CLIENTS.worldstream.example/heist/",
        "https://clients.worldstream.example:443/heist/",
        "https://user@clients.worldstream.example/heist/",
        "https://clients.worldstream.example/heist/?token=secret",
        "https://clients.worldstream.example/other/",
    ] {
        let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
        let mut candidate: serde_json::Value = serde_json::from_slice(&bootstrap(true))
            .unwrap_or_else(|error| unreachable!("bootstrap fixture: {error}"));
        candidate["deployments"][2]["surfaces"][0]["launch_url"] = json!(invalid);
        let candidate = serde_json::to_vec(&candidate)
            .unwrap_or_else(|error| unreachable!("bootstrap bytes: {error}"));
        assert!(
            ClientBindingStoreV1::open(
                &state.path().join("client-bindings"),
                &[release.as_slice()],
                &candidate,
            )
            .is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn semantic_pack_digest_is_authority_and_labels_are_diagnostic() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let root = state.path().join("client-bindings");
    let release = release("example.heist.web", "participant-web");
    let store = ClientBindingStoreV1::open(&root, &[release.as_slice()], &bootstrap(false))
        .unwrap_or_else(|error| unreachable!("store: {error}"));
    let mut relabeled = request(Some("navigator"));
    relabeled.pack.id = "diagnostic.publisher-label".to_owned();
    relabeled.pack.version = "2026.09-label".to_owned();

    assert!(matches!(
        store.select(&relabeled, None),
        Ok(ClientSelectionV1::Selected { candidate })
            if candidate.deployment_id == "heist-primary"
    ));
}

#[test]
fn disabled_binding_blocks_new_handoffs_but_not_its_active_client() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let root = state.path().join("client-bindings");
    let release = release("example.heist.web", "participant-web");
    let store = ClientBindingStoreV1::open(&root, &[release.as_slice()], &bootstrap(false))
        .unwrap_or_else(|error| unreachable!("store: {error}"));

    store
        .disable_binding("heist-primary-navigator")
        .unwrap_or_else(|error| unreachable!("disable: {error}"));
    assert!(matches!(
        store.select(&request(Some("navigator")), None),
        Ok(ClientSelectionV1::InspectorFallback { candidate })
            if candidate.candidate_id == "configured-inspector"
    ));
    assert!(matches!(
        store.resolve_active_client(
            &request(Some("navigator")),
            ClientCandidateClassV1::Binding,
            "heist-primary-navigator",
        ),
        Ok(candidate) if candidate.deployment_id == "heist-primary"
    ));
}

#[test]
fn revoked_deployment_blocks_new_handoffs_and_its_active_client() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let root = state.path().join("client-bindings");
    let release = release("example.heist.web", "participant-web");
    let store = ClientBindingStoreV1::open(&root, &[release.as_slice()], &bootstrap(false))
        .unwrap_or_else(|error| unreachable!("store: {error}"));

    store
        .revoke_deployment("heist-primary")
        .unwrap_or_else(|error| unreachable!("revoke: {error}"));
    assert!(matches!(
        store.select(&request(Some("navigator")), None),
        Ok(ClientSelectionV1::InspectorFallback { candidate })
            if candidate.candidate_id == "configured-inspector"
    ));
    assert!(matches!(
        store.resolve_active_client(
            &request(Some("navigator")),
            ClientCandidateClassV1::Binding,
            "heist-primary-navigator",
        ),
        Err(worldstream_studio_supervisor::client_bindings::ClientBindingStoreErrorV1::InvalidChoice)
    ));
}

#[test]
fn fresh_documented_imports_select_current_and_retained_archive_clients()
-> Result<(), Box<dyn std::error::Error>> {
    let configuration =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/activity-clients");
    for (declaration_name, origin, suffix) in [
        ("cli-import.json", "http://127.0.0.1:5173", "/"),
        (
            "hosted-local-import.json",
            "http://127.0.0.1:5180",
            "/hosted/",
        ),
    ] {
        let declaration =
            parse_client_declaration(&std::fs::read(configuration.join(declaration_name))?)?;
        let releases: Vec<Vec<u8>> = declaration
            .release_files
            .iter()
            .map(|file| std::fs::read(configuration.join(file)))
            .collect::<Result<_, _>>()?;
        let release_bytes: Vec<&[u8]> = releases.iter().map(Vec::as_slice).collect();
        let bindings = std::fs::read(configuration.join(declaration.bindings_file))?;
        let state = TempDir::new()?;
        let store = ClientBindingStoreV1::open(
            &state.path().join("client-bindings"),
            &release_bytes,
            &bindings,
        )?;
        for (generation, revision) in [
            (
                "v10",
                "blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64",
            ),
            (
                "v11",
                "blake3:26c51f969dc7949fb42556d013eeec555ae83dc9f28cb582c03f0541e420e776",
            ),
        ] {
            // Hosted v11 Listing and candidate wiring are delivered together in IMO-209.
            if generation == "v11" && declaration_name == "hosted-local-import.json" {
                continue;
            }
            let request = ClientSelectionRequestV1 {
                pack: ExactPackReferenceV1 {
                    id: "worldstream.midnight-archive".to_owned(),
                    version: "0.1.0".to_owned(),
                    digest: revision.to_owned(),
                },
                client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
                access_mode: AccessMode::Participant,
                role: Some("lead".to_owned()),
            };
            assert!(
                matches!(
                    store.select(&request, None),
                    Ok(ClientSelectionV1::Selected { candidate })
                        if candidate.launch_url == format!("{origin}/midnight-archive-{generation}{suffix}")
                ),
                "{declaration_name} must select the exact {generation} surface after a fresh import"
            );
        }
    }
    Ok(())
}

#[test]
fn loads_the_repository_client_host_configuration_without_pack_specific_code() {
    let state = TempDir::new().unwrap_or_else(|error| unreachable!("state: {error}"));
    let configuration =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/activity-clients");
    let store = ClientBindingStoreV1::open_configured(
        &state.path().join("client-bindings"),
        &configuration.join("releases"),
        &configuration.join("local-bindings.json"),
    )
    .unwrap_or_else(|error| unreachable!("configured store: {error}"));

    let heist_spectator = ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820"
                .to_owned(),
        },
        client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
        access_mode: AccessMode::Spectator,
        role: None,
    };
    assert!(matches!(
        store.select(&heist_spectator, None),
        Ok(ClientSelectionV1::Selected { candidate })
            if candidate.launch_url == "http://127.0.0.1:5173/agent-heist-v3/"
    ));

    let negotiate_participant = ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: "worldstream.negotiate".to_owned(),
            version: "0.1.0".to_owned(),
            digest: "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589"
                .to_owned(),
        },
        client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
        access_mode: AccessMode::Participant,
        role: Some("buyer_agent".to_owned()),
    };
    assert!(matches!(
        store.select(&negotiate_participant, None),
        Ok(ClientSelectionV1::Selected { candidate })
            if candidate.launch_url == "http://127.0.0.1:5173/negotiate/"
    ));

    for (digest, expected_url) in [
        (
            "blake3:679022bf13c15ea014e18a7129b679c9bfd27873c570fd9cd0c02818f0880a7a",
            "http://127.0.0.1:5173/midnight-archive-v1/",
        ),
        (
            "blake3:ee85f264b9c3dfb185ebedc9646bea655740f351c287793cce997336f0f419f2",
            "http://127.0.0.1:5173/midnight-archive-v2/",
        ),
        (
            "blake3:6c3ad825a65307b9f5434d4a9140b7db4bd70d1f7830c66d6f6af1d2ba9dc0da",
            "http://127.0.0.1:5173/midnight-archive-v3/",
        ),
        (
            "blake3:ec4689e090f05f1c1894f21c1dba95e1f56b3afc49fc88c5c8f3530a03a80b61",
            "http://127.0.0.1:5173/midnight-archive-v4/",
        ),
        (
            "blake3:894f7a58c01b0ca99ac29b1b84a9083bab7f0f858f0b33ce495413255cf91339",
            "http://127.0.0.1:5173/midnight-archive-v5/",
        ),
        (
            "blake3:59bb814b920b0eb6f8b8886f2d368052189c0f9263d6c36c91894f639c8e19f5",
            "http://127.0.0.1:5173/midnight-archive-v6/",
        ),
        (
            "blake3:c3349a5688bf3b45e1497ee5d304231bb5d53bc2ef31f8a012082fa9aac28676",
            "http://127.0.0.1:5173/midnight-archive-v7/",
        ),
        (
            "blake3:94d59090be49f1abd4dc7413ce52b26d07f12bedd4d427b0c24569f90837845b",
            "http://127.0.0.1:5173/midnight-archive-v8/",
        ),
        (
            "blake3:d398d13df28f50edcc271aa8f6ffa75f1c26eca1851ac78215aa8e0f6017d8e5",
            "http://127.0.0.1:5173/midnight-archive-v9/",
        ),
        (
            "blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64",
            "http://127.0.0.1:5173/midnight-archive-v10/",
        ),
    ] {
        let archive_participant = ClientSelectionRequestV1 {
            pack: ExactPackReferenceV1 {
                id: "worldstream.midnight-archive".to_owned(),
                version: "0.1.0".to_owned(),
                digest: digest.to_owned(),
            },
            client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
            access_mode: AccessMode::Participant,
            role: Some("lead".to_owned()),
        };
        assert!(matches!(
            store.select(&archive_participant, None),
            Ok(ClientSelectionV1::Selected { candidate })
                if candidate.launch_url == expected_url
        ));
    }

    let unknown_pack = ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: "example.unknown".to_owned(),
            version: "1.0.0".to_owned(),
            digest: PACK_DIGEST.to_owned(),
        },
        client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
        access_mode: AccessMode::Participant,
        role: Some("member".to_owned()),
    };
    assert!(matches!(
        store.select(&unknown_pack, None),
        Ok(ClientSelectionV1::InspectorFallback { candidate })
            if candidate.launch_url == "http://127.0.0.1:5173/inspector-v2/"
    ));
}
