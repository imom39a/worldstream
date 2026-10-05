// Live, isolated provider tests for the existing operator entrypoints.

#[test]
fn live_mixed_history_bundle_stream_and_native_postgres_restore()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::operator_transfer::{
        begin_transfer, export_pending_stream_v2, finalize_stream_authority_v2, finalize_transfer,
        import_stream_chunks_v2, resume_transfer,
    };
    use worldstream_core::{CanonicalHistoryFormat, CanonicalJsonV1, CompleteHeadV1};
    use worldstream_postgres::{PostgresAdmin, PostgresConnectionConfig};
    use worldstream_transfer::{
        BundleProfileV1, RecordParityV1, TargetFingerprintV1, TransferBundleV1,
        TransferStreamLimitsV2, TransferStreamReaderV2,
    };

    let Ok(dsn_file) = std::env::var("WORLDSTREAM_SCALING_POSTGRES_ADMIN_DSN_FILE") else {
        println!("LIVE_MIXED_OPERATORS=SKIP reason=dsn_unset");
        return Ok(());
    };
    let base_dsn = fs::read_to_string(dsn_file)?;
    let base: postgres::Config = base_dsn
        .trim()
        .parse()
        .map_err(|_| "isolated provider config")?;
    if !matches!(base.get_hosts(), [postgres::config::Host::Tcp(host)] if host == "127.0.0.1") {
        return Err("mixed operator test requires the owned loopback provider".into());
    }
    let mut provider = base
        .connect(postgres::NoTls)
        .map_err(|_| "isolated provider connection")?;
    let version: String = provider.query_one("SHOW server_version_num", &[])?.get(0);
    assert_eq!(version, "170011");
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| "operator fixture identity")?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root = std::env::var_os("WORLDSTREAM_SCALING_OPERATOR_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::temp_dir().join("worldstream-mixed-operators"));
    let directory = root.join(&suffix);
    fs::create_dir_all(&directory)?;
    prepare_test_directory(&directory)?;
    let registry = builtin_counter_registry()?;

    for lane in ["bundle", "stream"] {
        let lane_dir = directory.join(lane);
        fs::create_dir(&lane_dir)?;
        prepare_test_directory(&lane_dir)?;
        let source = lane_dir.join("mixed-source.sqlite3");
        let (_, rooms) = initialize_room_sources_and_companion(
            &source,
            &[CanonicalHistoryFormat::V1, CanonicalHistoryFormat::V2],
            true,
        )?;
        let original = extract_restore_evidence(&source, NativeSqliteLimits::default())?;
        let database = format!("worldstream_scaling_mixed_{lane}_{suffix}");
        provider.batch_execute(&format!("CREATE DATABASE {database}"))?;
        let target_dsn = format!("{} dbname={database}", base_dsn.trim());
        let admin =
            PostgresAdmin::new(PostgresConnectionConfig::direct_admin(target_dsn.clone())?)?;
        admin.migrate()?;
        let backup = lane_dir.join("source.backup.sqlite3");
        let artifact = lane_dir.join(format!("source.{lane}"));

        if lane == "bundle" {
            let state = lane_dir.join("state");
            fs::create_dir(&state)?;
            prepare_test_directory(&state)?;
            let backup = state.join("source.backup.sqlite3");
            let artifact = state.join("source.bundle");
            begin_transfer(
                &source,
                &backup,
                &artifact,
                &state,
                &format!("mixed/{suffix}"),
            )?;
            let first = resume_transfer(&source, &artifact, &state, &admin, 1)?;
            assert!(!first.chunks_complete);
            assert!(finalize_transfer(&source, &artifact, &state, &admin).is_err());
            // Reopen every invocation. Durable source/target journals are the retry authority.
            loop {
                if resume_transfer(&source, &artifact, &state, &admin, 3)?.chunks_complete {
                    break;
                }
            }
            let bundle = TransferBundleV1::from_bytes(&fs::read(&artifact)?)?;
            assert_eq!(bundle.target_profile(), BundleProfileV1::PostgresPrimary17);
            let complete = finalize_transfer(&source, &artifact, &state, &admin)?;
            assert_eq!(complete.phase, "target_authoritative");
            assert_eq!(
                finalize_transfer(&source, &artifact, &state, &admin)?,
                complete
            );
        } else {
            let store = SqliteRoomStore::open(&source)?;
            store.begin_source_transfer(&backup)?;
            let limits = TransferStreamLimitsV2::default();
            export_pending_stream_v2(&store, &artifact, &format!("mixed/{suffix}"), limits)?;
            let mut reader = TransferStreamReaderV2::new(File::open(&artifact)?, limits)?;
            let manifest = reader.manifest().ok_or("stream manifest")?.clone();
            let identity = reader.identity().clone();
            let mut records = Vec::new();
            while let Some(chunk) = reader.next_chunk()? {
                records.extend_from_slice(chunk.records());
            }
            let parity = RecordParityV1::from_records(&records)?;
            let pack = manifest
                .deployment_identity()
                .packs()
                .first()
                .ok_or("stream Pack identity")?
                .clone();
            let target = TargetFingerprintV1::observed(
                BundleProfileV1::PostgresPrimary17,
                identity.source_epoch() + 1,
                identity.lineage_id(),
                worldstream_postgres::postgres_backend_fingerprint()?,
                pack,
                manifest.deployment_identity().resources().to_vec(),
                reader.stream_header_digest(),
                parity,
            )?;
            import_stream_chunks_v2(&artifact, &admin, target.clone(), limits)?;
            import_stream_chunks_v2(&artifact, &admin, target.clone(), limits)?;
            let complete =
                finalize_stream_authority_v2(&store, &artifact, &admin, target.clone(), limits)?;
            assert_eq!(complete.target_epoch, 4);
            assert_eq!(
                finalize_stream_authority_v2(&store, &artifact, &admin, target, limits)?,
                complete
            );
            assert_eq!(
                store.source_transfer_status()?.state(),
                worldstream_sqlite::SqliteSourceTransferStateV1::SourceRetired
            );
        }
        for room in &rooms {
            let verified = admin.verify_room(room)?;
            verified.verify_executable_replay(&registry)?;
            let records = original
                .canonical_records
                .get(room)
                .ok_or("original records")?;
            assert_eq!(verified.genesis_bytes, records[0].bytes);
            assert_eq!(
                verified.transition_bytes,
                records
                    .iter()
                    .skip(1)
                    .map(|record| record.bytes.clone())
                    .collect::<Vec<_>>()
            );
            let head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(
                &original.room_heads[room].complete_head_bytes,
            )?;
            assert_eq!(verified.head, head);
            assert_eq!(
                (&verified.core_state_bytes, &verified.activity_state_bytes),
                (
                    &original.materializations[room].0,
                    &original.materializations[room].1
                )
            );
        }
        let source_store = SqliteRoomStore::open(&source)?;
        assert_eq!(
            source_store.source_transfer_status()?.state(),
            worldstream_sqlite::SqliteSourceTransferStateV1::SourceRetired
        );
        drop(source_store);
        if lane == "bundle" {
            mixed_native_postgres_round_trip(
                &base,
                &mut provider,
                &database,
                &target_dsn,
                &lane_dir,
                &rooms,
            )?;
        }
    }
    println!(
        "LIVE_MIXED_OPERATORS=PASS formats=v1,v2 bundle=pass stream=pass native_restore=pass records=exact receipts=verified"
    );
    Ok(())
}

fn mixed_native_postgres_round_trip(
    base: &postgres::Config,
    provider: &mut postgres::Client,
    database: &str,
    target_dsn: &str,
    lane_dir: &Path,
    rooms: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    use worldstream_postgres::{PostgresAdmin, PostgresConnectionConfig};
    let ctl =
        std::env::var_os("WORLDSTREAM_SCALING_CTL").ok_or("built operator binary is required")?;
    let tools = std::env::var_os("WORLDSTREAM_SCALING_POSTGRES_TOOLS")
        .ok_or("PostgreSQL17.11 tools are required")?;
    let tools = std::path::PathBuf::from(tools);
    let port = *base.get_ports().first().ok_or("provider port")?;
    let user = base.get_user().ok_or("provider user")?;
    let password = std::str::from_utf8(base.get_password().ok_or("provider credential")?)?;
    if password.contains([':', '\n', '\\']) {
        return Err("passfile fixture credential format".into());
    }
    let restored_database = format!("{database}_restore");
    provider.batch_execute(&format!(
        "CREATE DATABASE {restored_database} CONNECTION LIMIT 0"
    ))?;
    provider.batch_execute(&format!(
        "COMMENT ON DATABASE {restored_database} IS 'worldstream/native-postgres-disposable-target/v1'"
    ))?;
    let passfile = lane_dir.join("native.pgpass");
    write_owner_only(&passfile, format!("127.0.0.1:{port}:{database}:{user}:{password}\n127.0.0.1:{port}:{restored_database}:{user}:{password}\n").as_bytes())?;
    let artifacts = lane_dir.join("native");
    fs::create_dir(&artifacts)?;
    prepare_test_directory(&artifacts)?;
    let identity =
        worldstream_postgres::native_restore::native_postgres_artifact_directory_identity(
            &artifacts,
        )?;
    let rebuild = std::process::Command::new(&ctl)
        .args([
            "postgres",
            "snapshots",
            "rebuild",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--database",
            database,
            "--username",
            user,
            "--tls-mode",
            "disable",
            "--passfile",
        ])
        .arg(&passfile)
        .output()?;
    fs::write(lane_dir.join("native-rebuild.stdout.json"), &rebuild.stdout)?;
    fs::write(lane_dir.join("native-rebuild.stderr.txt"), &rebuild.stderr)?;
    assert!(
        rebuild.status.success(),
        "native snapshot rebuild failed; see retained redacted log"
    );
    let dump = artifacts.join("mixed.dump");
    let report = artifacts.join("mixed.report.json");
    let output = std::process::Command::new(&ctl)
        .args([
            "postgres",
            "native",
            "restore",
            "--source-host",
            "127.0.0.1",
            "--source-port",
            &port.to_string(),
            "--source-database",
            database,
            "--source-username",
            user,
            "--source-tls-mode",
            "disable",
            "--target-host",
            "127.0.0.1",
            "--target-port",
            &port.to_string(),
            "--target-database",
            &restored_database,
            "--target-username",
            user,
            "--target-tls-mode",
            "disable",
            "--passfile",
        ])
        .arg(&passfile)
        .arg("--pg-dump")
        .arg(tools.join("pg_dump"))
        .arg("--pg-restore")
        .arg(tools.join("pg_restore"))
        .arg("--psql")
        .arg(tools.join("psql"))
        .arg("--dump")
        .arg(&dump)
        .arg("--report")
        .arg(&report)
        .args([
            "--artifact-directory-storage-id",
            &identity.storage_id,
            "--artifact-directory-file-id",
            &identity.file_id,
            "--timeout-seconds",
            "180",
        ])
        .output()?;
    fs::write(lane_dir.join("native-restore.stdout.json"), &output.stdout)?;
    fs::write(lane_dir.join("native-restore.stderr.txt"), &output.stderr)?;
    assert!(
        output.status.success(),
        "native mixed restore failed; see retained redacted log"
    );
    let result: serde_json::Value = serde_json::from_slice(&fs::read(report)?)?;
    assert_eq!(result["exact_restored_row_set"], true);
    assert_eq!(result["native_dump_restore"], "pass");
    assert_eq!(
        result["source_durable_domains_digest"],
        result["restored_durable_domains_digest"]
    );
    assert!(fs::metadata(dump)?.len() > 0);
    let source_admin = PostgresAdmin::new(PostgresConnectionConfig::direct_admin(
        target_dsn.to_owned(),
    )?)?;
    let restored_dsn = format!("{target_dsn} dbname={restored_database}");
    let restored_admin = PostgresAdmin::new(PostgresConnectionConfig::direct_admin(restored_dsn)?)?;
    let registry = builtin_counter_registry()?;
    for room in rooms {
        let source = source_admin.verify_room(room)?;
        let restored = restored_admin.verify_room(room)?;
        restored.verify_executable_replay(&registry)?;
        assert_eq!(source.genesis_bytes, restored.genesis_bytes);
        assert_eq!(source.transition_bytes, restored.transition_bytes);
        assert_eq!(
            source.pack_revision_lock_bytes,
            restored.pack_revision_lock_bytes
        );
        assert_eq!(source.head, restored.head);
    }
    Ok(())
}
