#![allow(clippy::panic)]

use std::{collections::BTreeMap, fmt::Display, path::Path, str::FromStr, sync::Arc};

use serde::Serialize;
use serde_json::{Value, json};
use tempfile::tempdir;
use worldstream_core::{
    Blake3DigestV1, CanonicalJsonV1, NamedDigestV1, PackCodecBundleV1, PackDigestV1,
    PackRevisionLockV1, SchemaReferenceV1,
};

use crate::{
    ApprovalDecisionV1, BUNDLE_FORMAT_ID, BundleManifestV1, BundleMemberManifestV1,
    CANONICAL_CODEC_ID, CODEC_BUNDLE_MEMBER, CONFORMANCE_MEMBER, DEPENDENCY_LOCK_MEMBER,
    DESCRIPTOR_MEMBER, EXECUTION_PROFILE_ID, EXECUTOR_MEMBER, GOLDEN_CORPUS_MEMBER,
    HOST_CONTRACT_ID, MANIFEST_MEMBER, OperatorApprovalV1, PackBundleErrorV1, PackBundleStoreV1,
    PackBundleVerifierV1, PackBundleWriterV1, PackInstallStateV1, PackRemovalV1,
    PackStartupReadinessSealV1, REVISION_LOCK_MEMBER, RetainedPackBundleArtifactV1,
    RetainedRevisionSourceV1, SCHEMAS_MEMBER,
    manifest::{
        CONFORMANCE_ID, ConformanceDocumentV1, REVISION_LOCK_ID, SCHEMA_BUNDLE_DOMAIN,
        SchemaBundleDocumentV1, SchemaDocumentV1,
    },
    ustar::CanonicalUstarV1,
};

struct Fixture {
    bytes: Vec<u8>,
    component: Vec<u8>,
}

#[derive(Serialize)]
struct SchemaDigestInput<'a> {
    domain: &'static str,
    schemas: &'a [SchemaReferenceV1],
}

fn must<T, E: Display>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("test fixture must succeed: {error}"))
}

fn canonical<T: Serialize>(value: &T) -> Vec<u8> {
    let serialized = must(serde_json::to_vec(value));
    must(must(CanonicalJsonV1::parse(&serialized)).to_bytes())
}

fn pack_digest(digest: &Blake3DigestV1) -> PackDigestV1 {
    must(PackDigestV1::from_str(&digest.to_string()))
}

#[allow(clippy::too_many_lines)]
fn fixture(component_marker: &str) -> Fixture {
    let schema_value = json!({"additionalProperties": true, "type": "object"});
    let schema_bytes = canonical(&schema_value);
    let schema_digest = Blake3DigestV1::hash(&schema_bytes);
    let schema_reference = SchemaReferenceV1 {
        schema_digest: schema_digest.clone(),
        schema_id: "fixture/object/v1".to_owned(),
    };
    let schemas = SchemaBundleDocumentV1 {
        schemas: vec![SchemaDocumentV1 {
            canonical_schema: schema_value,
            schema_digest: schema_digest.clone(),
            schema_id: schema_reference.schema_id.clone(),
        }],
    };
    let schema_bundle_digest = Blake3DigestV1::hash(&canonical(&SchemaDigestInput {
        domain: SCHEMA_BUNDLE_DOMAIN,
        schemas: std::slice::from_ref(&schema_reference),
    }));

    let codecs = PackCodecBundleV1::canonical_v1();
    let codec_bundle_digest = must(codecs.digest());

    let reference = json!({
        "schema_digest": schema_digest,
        "schema_id": schema_reference.schema_id,
    });
    let mut descriptor_content = json!({
        "actions": [{"action_type": "act", "payload_schema": reference}],
        "attention_reasons": [],
        "canonical_codec": CANONICAL_CODEC_ID,
        "configuration_schema": reference,
        "event_schemas": {},
        "explanatory_version": "0.1.0-test",
        "host_contract": HOST_CONTRACT_ID,
        "limits": {
            "maximum_attention_signals": 4,
            "maximum_collection_items": 128,
            "maximum_events": 4,
            "maximum_nesting": 16,
            "maximum_observation_bytes": 65536,
            "maximum_projection_bytes": 65536,
            "maximum_state_bytes": 65536,
            "maximum_text_bytes": 8192,
            "maximum_timer_requests": 4
        },
        "name": "Bundle Fixture",
        "observation_schemas": {
            "final_reveal": reference,
            "historical_operator": reference,
            "historical_participant": reference,
            "historical_public": reference,
            "operator": reference,
            "participant": reference,
            "public": reference
        },
        "output_schemas": {},
        "pack_id": "worldstream.fixture",
        "projection_schemas": {
            "final_reveal": reference,
            "historical_operator": reference,
            "historical_participant": reference,
            "historical_public": reference,
            "operator": reference,
            "participant": reference,
            "public": reference
        },
        "rejection_codes": [],
        "roles": [{"maximum": 1, "minimum": 1, "role": "member"}],
        "state_schema": reference,
        "stimulus_schemas": {}
    });
    let descriptor_digest = Blake3DigestV1::hash(&canonical(&descriptor_content));
    let component = format!("\0asm-worldstream-{component_marker}").into_bytes();
    let dependency_lock = canonical(&json!({
        "dependency_lock_id": "worldstream/typescript-pack-dependency-lock/v1",
        "packages": [],
        "toolchain": {"fixture": "1"}
    }));
    let static_bytes = canonical(&json!({"calibration": 1}));
    let lock = PackRevisionLockV1 {
        revision_lock_id: REVISION_LOCK_ID.to_owned(),
        pack_id: "worldstream.fixture".to_owned(),
        explanatory_version: "0.1.0-test".to_owned(),
        host_contract: HOST_CONTRACT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        descriptor_digest,
        schema_bundle_digest,
        codec_bundle_digest,
        deterministic_static_data_digests: vec![NamedDigestV1 {
            digest: Blake3DigestV1::hash(&static_bytes),
            name: "static/calibration.json".to_owned(),
        }],
        rule_source_digest: Blake3DigestV1::hash(&component),
        deterministic_dependency_lock_digest: Blake3DigestV1::hash(&dependency_lock),
    };
    let lock_bytes = canonical(&lock);
    let revision_digest = pack_digest(&Blake3DigestV1::hash(&lock_bytes));
    descriptor_content
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("fixture descriptor is an object"))
        .insert(
            "revision_digest".to_owned(),
            Value::String(revision_digest.to_string()),
        );
    let descriptor = canonical(&descriptor_content);
    let golden = canonical(&json!({
        "actions": [],
        "corpus_id": "worldstream/pack-golden-corpus/v1",
        "expected_transcript_digest": Blake3DigestV1::hash(b"fixture transcript"),
        "genesis": {
            "configuration": {},
            "created_at": "2026-08-30T12:00:00Z",
            "initial_core_state": {"memberships": {}, "room_status": "active"},
            "pack_digest": revision_digest,
            "room_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "room_seed": "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        },
        "viewers": []
    }));
    let conformance = canonical(&ConformanceDocumentV1 {
        conformance_id: CONFORMANCE_ID.to_owned(),
        execution_profile_id: EXECUTION_PROFILE_ID.to_owned(),
        executor_component_digest: Blake3DigestV1::hash(&component),
        golden_corpus_digest: Blake3DigestV1::hash(&golden),
        passed: true,
        revision_digest: revision_digest.clone(),
    });

    let mut members = BTreeMap::from([
        (CODEC_BUNDLE_MEMBER.to_owned(), canonical(&codecs)),
        (CONFORMANCE_MEMBER.to_owned(), conformance),
        (DEPENDENCY_LOCK_MEMBER.to_owned(), dependency_lock),
        (DESCRIPTOR_MEMBER.to_owned(), descriptor),
        (EXECUTOR_MEMBER.to_owned(), component.clone()),
        (GOLDEN_CORPUS_MEMBER.to_owned(), golden),
        (REVISION_LOCK_MEMBER.to_owned(), lock_bytes),
        (SCHEMAS_MEMBER.to_owned(), canonical(&schemas)),
        ("static/calibration.json".to_owned(), static_bytes),
    ]);
    let declared_members = members
        .iter()
        .map(|(name, bytes)| BundleMemberManifestV1 {
            blake3: Blake3DigestV1::hash(bytes),
            name: name.clone(),
            size: u64::try_from(bytes.len())
                .unwrap_or_else(|_| unreachable!("fixture member length fits u64")),
        })
        .collect();
    let manifest = BundleManifestV1 {
        bundle_format_id: BUNDLE_FORMAT_ID.to_owned(),
        canonical_codec: CANONICAL_CODEC_ID.to_owned(),
        execution_profile_id: EXECUTION_PROFILE_ID.to_owned(),
        host_contract: HOST_CONTRACT_ID.to_owned(),
        members: declared_members,
        revision_digest,
        static_members: vec!["static/calibration.json".to_owned()],
    };
    members.insert(MANIFEST_MEMBER.to_owned(), canonical(&manifest));
    Fixture {
        bytes: must(PackBundleWriterV1::write(&members)),
        component,
    }
}

fn write_fixture(path: &Path, fixture: &Fixture) {
    must(std::fs::write(path, &fixture.bytes));
}

struct References(bool);

impl RetainedRevisionSourceV1 for References {
    fn is_revision_referenced(
        &self,
        _revision_digest: &PackDigestV1,
    ) -> Result<bool, PackBundleErrorV1> {
        Ok(self.0)
    }
}

#[test]
fn canonical_archive_and_complete_bundle_are_byte_stable() {
    let fixture = fixture("stable");
    let verified = must(PackBundleVerifierV1.inspect(Arc::from(fixture.bytes.clone())));
    assert_eq!(verified.archive_bytes(), fixture.bytes);
    assert_eq!(verified.component_bytes(), fixture.component);
    assert_eq!(verified.inspection().member_count, 10);

    let parsed = must(CanonicalUstarV1::parse(Arc::from(fixture.bytes.clone())));
    let members = parsed
        .member_names()
        .map(|name| {
            (
                name.to_owned(),
                parsed
                    .member(name)
                    .unwrap_or_else(|| unreachable!("listed member remains readable"))
                    .to_vec(),
            )
        })
        .collect();
    assert_eq!(must(PackBundleWriterV1::write(&members)), fixture.bytes);
}

#[test]
fn writer_and_verifier_reject_unsafe_metadata_and_substitution() {
    let unsafe_members = BTreeMap::from([("../escape".to_owned(), Vec::new())]);
    assert!(matches!(
        PackBundleWriterV1::write(&unsafe_members),
        Err(PackBundleErrorV1::UnsafeMemberPath(_))
    ));

    let tamper_fixture = fixture("tamper");
    let mut metadata_tamper = tamper_fixture.bytes.clone();
    metadata_tamper[100] = b'1';
    assert!(matches!(
        PackBundleVerifierV1.inspect(Arc::from(metadata_tamper)),
        Err(PackBundleErrorV1::ArchiveMetadataInvalid)
    ));

    let mut component_tamper = tamper_fixture.bytes;
    let component_at = component_tamper
        .windows(tamper_fixture.component.len())
        .position(|window| window == tamper_fixture.component)
        .unwrap_or_else(|| unreachable!("fixture Component is present"));
    component_tamper[component_at] ^= 1;
    assert!(matches!(
        PackBundleVerifierV1.inspect(Arc::from(component_tamper)),
        Err(PackBundleErrorV1::MemberDigestMismatch(name)) if name == EXECUTOR_MEMBER
    ));

    let fixture = fixture("noncanonical-json");
    let parsed = must(CanonicalUstarV1::parse(Arc::from(fixture.bytes)));
    let mut members: BTreeMap<_, _> = parsed
        .member_names()
        .map(|name| {
            (
                name.to_owned(),
                parsed
                    .member(name)
                    .unwrap_or_else(|| unreachable!("listed member remains readable"))
                    .to_vec(),
            )
        })
        .collect();
    members
        .get_mut(MANIFEST_MEMBER)
        .unwrap_or_else(|| unreachable!("fixture manifest exists"))
        .push(b'\n');
    let noncanonical_json = must(PackBundleWriterV1::write(&members));
    assert!(matches!(
        PackBundleVerifierV1.inspect(Arc::from(noncanonical_json)),
        Err(PackBundleErrorV1::InvalidTypedMember {
            member: MANIFEST_MEMBER,
            ..
        })
    ));

    members
        .get_mut(MANIFEST_MEMBER)
        .unwrap_or_else(|| unreachable!("fixture manifest exists"))
        .pop();
    members.remove(DEPENDENCY_LOCK_MEMBER);
    let missing_required = must(PackBundleWriterV1::write(&members));
    assert!(matches!(
        PackBundleVerifierV1.inspect(Arc::from(missing_required)),
        Err(PackBundleErrorV1::MissingMember(DEPENDENCY_LOCK_MEMBER))
    ));
}

#[test]
fn writer_round_trips_a_range_of_safe_member_maps() {
    for count in 1..=32 {
        let members: BTreeMap<_, _> = (0..count)
            .map(|index| {
                (
                    format!("static/{index:03}.bin"),
                    vec![u8::try_from(index).unwrap_or(0); index + 1],
                )
            })
            .collect();
        let first = must(PackBundleWriterV1::write(&members));
        let second = must(PackBundleWriterV1::write(&members));
        assert_eq!(first, second);
        let parsed = must(CanonicalUstarV1::parse(Arc::from(first)));
        assert_eq!(parsed.member_names().len(), count);
    }
}

#[test]
fn approval_install_export_revocation_and_safe_removal_are_fail_closed() {
    let temporary = must(tempdir());
    let candidate = temporary.path().join("candidate.wspack");
    let fixture = fixture("lifecycle");
    write_fixture(&candidate, &fixture);
    let store_root = temporary.path().join("activity-packs");
    let store = must(PackBundleStoreV1::open(&store_root));
    let inspection = must(store.inspect_path(&candidate));

    assert!(matches!(
        store.install_approved(&candidate, "2026-08-30T12:00:00Z"),
        Err(PackBundleErrorV1::ApprovalMissing)
    ));
    assert_eq!(
        must(std::fs::read_dir(store_root.join("objects").join("blake3"))).count(),
        0
    );
    assert_eq!(
        must(std::fs::read_dir(store_root.join("inventory"))).count(),
        0
    );
    must(store.approve_path(
        &candidate,
        OperatorApprovalV1 {
            operator_id: "operator-1".to_owned(),
            decided_at: "2026-08-30T12:01:00Z".to_owned(),
            decision: ApprovalDecisionV1::Approved,
        },
    ));
    let installed = must(store.install_approved(&candidate, "2026-08-30T12:02:00Z"));
    assert_eq!(installed.install_state, PackInstallStateV1::RetainedOnly);
    assert_eq!(installed.bundle_digest, inspection.bundle_digest);
    let selectable = must(store.set_selectable(&installed.bundle_digest, true));
    assert_eq!(selectable.install_state, PackInstallStateV1::Selectable);

    let mut exported = Vec::new();
    must(store.export_exact(&installed.bundle_digest, &mut exported));
    assert_eq!(exported, fixture.bytes);
    assert!(!String::from_utf8_lossy(&exported).contains("operator-1"));

    let verified = must(store.load_installed(&installed.bundle_digest));
    must(store.record_approval(
        &verified,
        OperatorApprovalV1 {
            operator_id: "operator-1".to_owned(),
            decided_at: "2026-08-30T12:03:00Z".to_owned(),
            decision: ApprovalDecisionV1::Revoked,
        },
    ));
    assert!(store.load_installed(&installed.bundle_digest).is_ok());
    assert!(matches!(
        store.set_selectable(&installed.bundle_digest, true),
        Err(PackBundleErrorV1::ApprovalDigestMismatch)
    ));
    assert!(matches!(
        store.remove_if_unreferenced(
            &installed.bundle_digest,
            &References(true),
            PackRemovalV1 {
                operator_id: "operator-1".to_owned(),
                removed_at: "2026-08-30T12:04:00Z".to_owned(),
            },
        ),
        Err(PackBundleErrorV1::RevisionReferenced)
    ));
    must(store.remove_if_unreferenced(
        &installed.bundle_digest,
        &References(false),
        PackRemovalV1 {
            operator_id: "operator-1".to_owned(),
            removed_at: "2026-08-30T12:05:00Z".to_owned(),
        },
    ));
    assert!(matches!(
        store.load_installed(&installed.bundle_digest),
        Err(PackBundleErrorV1::NotInstalled)
    ));
}

#[test]
fn retained_restore_reverifies_exact_bytes_and_never_copies_approval() {
    let temporary = must(tempdir());
    let store_root = temporary.path().join("activity-packs");
    let store = must(PackBundleStoreV1::open(&store_root));
    let fixture = fixture("retained-restore");
    let artifact = must(RetainedPackBundleArtifactV1::from_archive_bytes(
        fixture.bytes.clone(),
    ));

    let installed = must(store.restore_retained(&artifact, "2026-08-30T12:00:00Z"));
    assert_eq!(installed.install_state, PackInstallStateV1::RetainedOnly);
    assert_eq!(
        must(store.load_installed(&installed.bundle_digest)).archive_bytes(),
        fixture.bytes
    );
    assert_eq!(must(store.load_startup_inventory()).counts().selectable, 0);
    assert_eq!(
        must(std::fs::read_dir(store_root.join("approvals"))).count(),
        0
    );

    let mut tampered = artifact;
    tampered.archive_bytes.push(0);
    assert!(
        store
            .restore_retained(&tampered, "2026-08-30T12:00:01Z")
            .is_err()
    );
}

#[test]
fn startup_inventory_is_bounded_sorted_and_frozen_until_reloaded() {
    let temporary = must(tempdir());
    let root = temporary.path().join("activity-packs");
    let store = must(PackBundleStoreV1::open(&root));
    let first_path = temporary.path().join("first.wspack");
    let second_path = temporary.path().join("second.wspack");
    write_fixture(&first_path, &fixture("inventory-first"));
    write_fixture(&second_path, &fixture("inventory-second"));

    for (path, decided_at, installed_at) in [
        (&first_path, "2026-08-30T13:00:00Z", "2026-08-30T13:01:00Z"),
        (&second_path, "2026-08-30T13:02:00Z", "2026-08-30T13:03:00Z"),
    ] {
        must(store.approve_path(
            path,
            OperatorApprovalV1 {
                operator_id: "operator-1".to_owned(),
                decided_at: decided_at.to_owned(),
                decision: ApprovalDecisionV1::Approved,
            },
        ));
        let installed = must(store.install_approved(path, installed_at));
        if path == &first_path {
            must(store.set_selectable(&installed.bundle_digest, true));
            let frozen = must(store.load_startup_inventory());
            assert_eq!(frozen.entries().len(), 1);
            assert_eq!(frozen.counts().selectable, 1);
            assert_eq!(frozen.counts().retained_only, 0);

            must(store.set_selectable(&installed.bundle_digest, false));
            assert_eq!(frozen.counts().selectable, 1);
        }
    }

    let inventory = must(store.load_startup_inventory());
    assert_eq!(inventory.counts().installed, 2);
    assert_eq!(inventory.counts().selectable, 0);
    assert_eq!(inventory.counts().retained_only, 2);
    assert!(
        inventory
            .entries()
            .windows(2)
            .all(|pair| { pair[0].installed().bundle_digest < pair[1].installed().bundle_digest })
    );
    assert!(
        inventory.entries().iter().all(|entry| {
            entry.installed().revision_digest == *entry.bundle().revision_digest()
        })
    );
}

#[test]
fn startup_inventory_rejects_corruption_and_overflow() {
    let temporary = must(tempdir());
    let root = temporary.path().join("activity-packs");
    let store = must(PackBundleStoreV1::open(&root));
    let candidate = temporary.path().join("candidate.wspack");
    let fixture = fixture("inventory-corrupt");
    write_fixture(&candidate, &fixture);
    must(store.approve_path(
        &candidate,
        OperatorApprovalV1 {
            operator_id: "operator-1".to_owned(),
            decided_at: "2026-08-30T14:00:00Z".to_owned(),
            decision: ApprovalDecisionV1::Approved,
        },
    ));
    let installed = must(store.install_approved(&candidate, "2026-08-30T14:01:00Z"));
    let object = root
        .join("objects")
        .join("blake3")
        .join(installed.bundle_digest.path_component())
        .join("bundle.wspack");
    let mut corrupt = fixture.bytes;
    corrupt[512] ^= 1;
    must(std::fs::write(object, corrupt));
    assert!(matches!(
        store.load_startup_inventory(),
        Err(PackBundleErrorV1::CorruptInstalledObject)
    ));

    let overflow_root = temporary.path().join("overflow");
    let overflow_store = must(PackBundleStoreV1::open(&overflow_root));
    let inventory_root = overflow_root.join("inventory");
    for index in 0..=crate::MAX_INSTALLED_BUNDLE_COUNT {
        must(std::fs::write(
            inventory_root.join(format!("{index:064x}.json")),
            b"{}",
        ));
    }
    assert!(matches!(
        overflow_store.load_startup_inventory(),
        Err(PackBundleErrorV1::LimitExceeded)
    ));
}

#[test]
fn readiness_seal_is_validated_and_inventory_mutation_clears_it() {
    let temporary = must(tempdir());
    let root = temporary.path().join("activity-packs");
    let store = must(PackBundleStoreV1::open(&root));
    let candidate = temporary.path().join("candidate.wspack");
    write_fixture(&candidate, &fixture("readiness-seal"));
    must(store.approve_path(
        &candidate,
        OperatorApprovalV1 {
            operator_id: "operator-1".to_owned(),
            decided_at: "2026-08-30T14:00:00Z".to_owned(),
            decision: ApprovalDecisionV1::Approved,
        },
    ));
    let installed = must(store.install_approved(&candidate, "2026-08-30T14:01:00Z"));
    let seal = must(PackStartupReadinessSealV1::new(
        installed.bundle_digest.to_string(),
        "sqlite-bundled".to_owned(),
        installed.bundle_digest.to_string(),
    ));
    must(store.record_startup_readiness(&seal));
    assert_eq!(must(store.startup_readiness()), Some(seal));

    must(store.set_selectable(&installed.bundle_digest, true));
    assert_eq!(must(store.startup_readiness()), None);

    assert!(matches!(
        PackStartupReadinessSealV1::new(
            installed.bundle_digest.to_string(),
            "unsupported".to_owned(),
            installed.bundle_digest.to_string(),
        ),
        Err(PackBundleErrorV1::StartupReadinessMismatch)
    ));
    must(std::fs::write(
        root.join("restart-readiness-v1.json"),
        b"{\"deployment_binding\":\"blake3:0000000000000000000000000000000000000000000000000000000000000000\",\"inventory_digest\":\"blake3:0000000000000000000000000000000000000000000000000000000000000000\",\"readiness_record_id\":\"wrong\",\"storage_profile\":\"sqlite-bundled\"}",
    ));
    assert!(matches!(
        store.startup_readiness(),
        Err(PackBundleErrorV1::StartupReadinessMismatch)
    ));
}

#[test]
fn approval_is_bound_to_one_physical_digest_and_corruption_is_detected() {
    let temporary = must(tempdir());
    let first_path = temporary.path().join("first.wspack");
    let second_path = temporary.path().join("second.wspack");
    let first = fixture("first");
    let second = fixture("second");
    write_fixture(&first_path, &first);
    write_fixture(&second_path, &second);
    let root = temporary.path().join("activity-packs");
    let store = must(PackBundleStoreV1::open(&root));
    must(store.approve_path(
        &first_path,
        OperatorApprovalV1 {
            operator_id: "operator-1".to_owned(),
            decided_at: "2026-08-30T12:00:00Z".to_owned(),
            decision: ApprovalDecisionV1::Approved,
        },
    ));
    assert!(matches!(
        store.install_approved(&second_path, "2026-08-30T12:01:00Z"),
        Err(PackBundleErrorV1::ApprovalMissing)
    ));
    let installed = must(store.install_approved(&first_path, "2026-08-30T12:02:00Z"));
    let object = root
        .join("objects")
        .join("blake3")
        .join(installed.bundle_digest.path_component())
        .join("bundle.wspack");
    let mut corrupt = first.bytes;
    corrupt[512] ^= 1;
    must(std::fs::write(object, corrupt));
    assert!(matches!(
        store.load_installed(&installed.bundle_digest),
        Err(PackBundleErrorV1::CorruptInstalledObject)
    ));
}
