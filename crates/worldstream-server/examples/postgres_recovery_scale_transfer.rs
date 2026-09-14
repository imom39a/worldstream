//! Transfer-backed PostgreSQL 100k bounded-recovery qualification fixture.
//!
//! The source is built by the ordinary SQLite qualification driver. This
//! program transfers it through the public v2 whole-deployment coordinator,
//! rebuilds one PostgreSQL checkpoint only after a full verified replay, and
//! measures a separate ordinary cold recovery from that checkpoint.

use std::{
    collections::BTreeSet,
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Instant,
};

use anyhow::{Context as _, Result, anyhow, bail};
use postgres::{Client, NoTls};
use serde_json::json;
use worldstream_core::{
    Blake3DigestV1, CanonicalJsonV1, CompleteHeadV1, MemberId, RoomCheckpointOperationalWitnessV3,
    RoomRecoveryStorageV1, builtin_counter_registry, recover_room_from_storage_with_receipt,
};
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore,
    postgres_backend_fingerprint,
};
use worldstream_server::operator_transfer::{
    export_pending_stream_v2, finalize_stream_authority_v2,
};
use worldstream_sqlite::{SqliteRoomStore, SqliteSourceTransferStateV1};
use worldstream_transfer::{DigestV1, RecordParityV1, TargetFingerprintV1, TransferStreamLimitsV2};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const SCALE: u64 = 100_000;

struct Arguments {
    source: PathBuf,
    backup: PathBuf,
    stream: PathBuf,
    output: PathBuf,
    admin_dsn_file: PathBuf,
    runtime_dsn_file: PathBuf,
    stream_id: String,
}

fn arguments() -> Result<Arguments> {
    let mut source = None;
    let mut backup = None;
    let mut stream = None;
    let mut output = None;
    let mut admin_dsn_file = None;
    let mut runtime_dsn_file = None;
    let mut stream_id = None;
    let mut values = env::args_os().skip(1);
    while let Some(flag) = values.next() {
        let Some(value) = values.next() else {
            bail!("{flag:?} requires a value");
        };
        match flag.to_string_lossy().as_ref() {
            "--source" => source = Some(PathBuf::from(value)),
            "--backup" => backup = Some(PathBuf::from(value)),
            "--stream" => stream = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--admin-dsn-file" => admin_dsn_file = Some(PathBuf::from(value)),
            "--runtime-dsn-file" => runtime_dsn_file = Some(PathBuf::from(value)),
            "--stream-id" => stream_id = Some(value.to_string_lossy().into_owned()),
            _ => bail!("unknown argument {flag:?}"),
        }
    }
    Ok(Arguments {
        source: source.ok_or_else(|| anyhow!("missing --source"))?,
        backup: backup.ok_or_else(|| anyhow!("missing --backup"))?,
        stream: stream.ok_or_else(|| anyhow!("missing --stream"))?,
        output: output.ok_or_else(|| anyhow!("missing --output"))?,
        admin_dsn_file: admin_dsn_file.ok_or_else(|| anyhow!("missing --admin-dsn-file"))?,
        runtime_dsn_file: runtime_dsn_file.ok_or_else(|| anyhow!("missing --runtime-dsn-file"))?,
        stream_id: stream_id.ok_or_else(|| anyhow!("missing --stream-id"))?,
    })
}

fn read_dsn(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path).context("read DSN metadata")?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > 64 * 1024
    {
        bail!("DSN file is not an owner-supplied regular bounded file");
    }
    let value = fs::read_to_string(path).context("read DSN file")?;
    let value = value.trim().to_owned();
    if value.is_empty() || value.as_bytes().contains(&0) {
        bail!("DSN file is empty or malformed");
    }
    Ok(value)
}

fn limits() -> TransferStreamLimitsV2 {
    TransferStreamLimitsV2 {
        max_records_per_chunk: 64,
        max_chunk_bytes: 256 * 1024,
        max_record_bytes: 64 * 1024,
        max_total_records: 2_000_000,
        max_total_record_bytes: 4 * 1024 * 1024 * 1024,
    }
}

fn target_for_stream(lineage: String, source_epoch: u64) -> Result<TargetFingerprintV1> {
    Ok(TargetFingerprintV1::observed_canonical_export(
        source_epoch
            .checked_add(1)
            .ok_or_else(|| anyhow!("target epoch overflow"))?,
        lineage,
        postgres_backend_fingerprint()?,
        Vec::new(),
        DigestV1::hash(b"imo-222-postgres-recovery-scale-target-fence-v1"),
        RecordParityV1::from_records(&[])?,
    )?)
}

fn rss_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

fn publish(path: &Path, report: &serde_json::Value) -> Result<()> {
    if path.exists() {
        bail!("output already exists");
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(serde_json::to_string(report)?.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn run(arguments: Arguments) -> Result<serde_json::Value> {
    let source =
        SqliteRoomStore::open(&arguments.source).context("open SQLite qualification source")?;
    if source.source_transfer_state() != SqliteSourceTransferStateV1::SourceAuthoritative {
        bail!("source must be authoritative before scale transfer");
    }
    let pending = source
        .begin_source_transfer(&arguments.backup)
        .context("freeze source into a verified retained backup")?;
    let source_epoch = pending
        .source_epoch()
        .ok_or_else(|| anyhow!("source epoch absent"))?;
    let target = target_for_stream(source.deployment_lineage()?, source_epoch)?;
    export_pending_stream_v2(&source, &arguments.stream, &arguments.stream_id, limits())
        .context("export public v2 stream")?;

    let admin_dsn = read_dsn(&arguments.admin_dsn_file)?;
    let admin = PostgresAdmin::new(PostgresConnectionConfig::direct_admin(admin_dsn)?)?;
    admin.migrate().context("migrate PostgreSQL scale target")?;
    finalize_stream_authority_v2(&source, &arguments.stream, &admin, target, limits())
        .context("finalize public v2 whole-deployment transfer")?;
    if source.source_transfer_state() != SqliteSourceTransferStateV1::SourceRetired {
        bail!("source was not retired by the transfer authority handoff");
    }

    let runtime_dsn = read_dsn(&arguments.runtime_dsn_file)?;
    let runtime = PostgresRoomStore::new(PostgresConnectionConfig::runtime(
        runtime_dsn.clone(),
        PostgresConnectionPath::Direct,
    )?)?;
    let registry = builtin_counter_registry()?;
    let expected = runtime
        .rebuild_verified_recovery_checkpoint(&registry, ROOM)
        .context("full verified replay and checkpoint capture")?
        .ok_or_else(|| anyhow!("transferred qualification Room is missing"))?;
    if expected.head().room_seq().get() != SCALE {
        bail!("transferred Room has an unexpected Head sequence");
    }

    let candidate = RoomRecoveryStorageV1::inspect_recovery_candidate(&runtime, &ROOM.parse()?)?
        .ok_or_else(|| anyhow!("checkpoint candidate is missing after verified capture"))?;
    if !candidate.has_checkpoint() || candidate.tail_transition_count() > 250 {
        bail!("recovery candidate is not bounded by a verified checkpoint");
    }
    let recovery_started = Instant::now();
    let execution = recover_room_from_storage_with_receipt(&runtime, &registry, &ROOM.parse()?)?
        .ok_or_else(|| anyhow!("Room disappeared during measured recovery"))?;
    let recovery_ms = recovery_started.elapsed().as_millis();
    let receipt = execution.receipt();
    if !receipt.used_checkpoint()
        || receipt.checkpoint_room_seq().map(|value| value.get()) != Some(SCALE)
        || receipt.prefix_transition_records_delivered() != 0
        || receipt.prefix_transitions_skipped() != SCALE
        || receipt.tail_transition_records_delivered()
            != u64::try_from(candidate.tail_transition_count())?
    {
        bail!("measured recovery did not complete through the checkpoint path");
    }
    let recovered = execution.into_trace();
    if recovered.head() != expected.head()
        || recovered.core_state().canonical_bytes()? != expected.core_state().canonical_bytes()?
        || recovered.activity_state().to_bytes()? != expected.activity_state().to_bytes()?
        || recovered.activity_callback_count()
            != usize::try_from(receipt.tail_transition_records_delivered())?
    {
        bail!("bounded recovery differs from the verified full replay");
    }

    let mut client =
        Client::connect(&runtime_dsn, NoTls).context("open runtime scale evidence reader")?;
    let checkpoint = client.query_one(
        "SELECT snapshots.room_seq, snapshots.complete_head_bytes, witness.witness_hash, witness.witness_bytes \
         FROM worldstream_room_snapshots AS snapshots JOIN worldstream_room_snapshot_operational_witnesses_v3 AS witness \
         ON witness.room_id = snapshots.room_id AND witness.room_seq = snapshots.room_seq \
         WHERE snapshots.room_id = $1 ORDER BY snapshots.room_seq DESC LIMIT 1",
        &[&ROOM],
    )?;
    let checkpoint_seq: i64 = checkpoint.get(0);
    let checkpoint_head_bytes: Vec<u8> = checkpoint.get(1);
    let witness_hash: Vec<u8> = checkpoint.get(2);
    let witness_bytes: Vec<u8> = checkpoint.get(3);
    let checkpoint_head =
        CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&checkpoint_head_bytes)?;
    let witness =
        RoomCheckpointOperationalWitnessV3::from_canonical_bytes(&witness_bytes, &checkpoint_head)
            .map_err(|error| anyhow!("decode operational witness: {error:?}"))?;
    if checkpoint_seq != i64::try_from(SCALE)?
        || checkpoint_head != *expected.head()
        || witness.checkpoint_head() != expected.head()
        || witness_hash.as_slice() != Blake3DigestV1::hash(&witness_bytes).as_bytes()
        || witness.canonical_bytes()? != witness_bytes
    {
        bail!("captured checkpoint witness does not bind the verified Head");
    }
    let counts = client.query_one(
        "SELECT (SELECT count(*) FROM worldstream_transitions WHERE room_id = $1), \
                (SELECT count(*) FROM worldstream_timers WHERE room_id = $1), \
                (SELECT count(*) FROM worldstream_frames WHERE room_id = $1), \
                (SELECT count(*) FROM worldstream_observation_consequences WHERE room_id = $1), \
                (SELECT count(*) FROM worldstream_activation_decisions WHERE room_id = $1), \
                (SELECT count(*) FROM worldstream_members WHERE room_id = $1), \
                (SELECT count(*) FROM worldstream_semantic_receipts WHERE room_id = $1)",
        &[&ROOM],
    )?;
    let transition_rows: i64 = counts.get(0);
    let timer_rows: i64 = counts.get(1);
    let frame_rows: i64 = counts.get(2);
    let consequence_rows: i64 = counts.get(3);
    let activation_rows: i64 = counts.get(4);
    let membership_rows: i64 = counts.get(5);
    let semantic_receipt_rows: i64 = counts.get(6);
    let operational_roots = client.query(
        "SELECT roots.domain, roots.entry_count, roots.root_hash, receipts.leaf_count, receipts.root_hash \
         FROM worldstream_room_operational_history_roots_v2 AS roots \
         JOIN worldstream_room_operational_mmr_receipts_v1 AS receipts \
           ON receipts.room_id = roots.room_id AND receipts.domain = roots.domain \
         WHERE roots.room_id = $1 ORDER BY roots.domain",
        &[&ROOM],
    )?;
    let expected_operational_counts = [
        ("frames", frame_rows),
        ("consequences", consequence_rows),
        ("activation_decisions", activation_rows),
    ];
    let mut observed_domains = BTreeSet::new();
    for row in &operational_roots {
        let domain: String = row.get(0);
        let entry_count: i64 = row.get(1);
        let root_hash: Vec<u8> = row.get(2);
        let mmr_leaf_count: i64 = row.get(3);
        let mmr_root_hash: Vec<u8> = row.get(4);
        let expected_count = expected_operational_counts
            .iter()
            .find_map(|(expected_domain, count)| (domain == *expected_domain).then_some(*count))
            .ok_or_else(|| anyhow!("unexpected operational root domain"))?;
        let witness_root = witness
            .operational_history_roots()
            .get(&domain)
            .ok_or_else(|| anyhow!("checkpoint witness is missing an operational root"))?;
        let witness_mmr = witness
            .operational_mmr_receipts()
            .get(&domain)
            .ok_or_else(|| anyhow!("checkpoint witness is missing an operational MMR receipt"))?;
        if !observed_domains.insert(domain.clone())
            || entry_count != expected_count
            || mmr_leaf_count != expected_count
            || u64::try_from(entry_count)? != witness_root.entry_count()
            || root_hash.as_slice() != witness_root.root_hash().as_bytes()
            || u64::try_from(mmr_leaf_count)? != witness_mmr.leaf_count()
            || mmr_root_hash.as_slice() != witness_mmr.root_hash().as_bytes()
        {
            bail!("captured witness does not match transferred operational roots");
        }
    }
    let members = client.query(
        "SELECT member_id, frame_head, membership_generation FROM worldstream_members \
         WHERE room_id = $1 ORDER BY member_id",
        &[&ROOM],
    )?;
    for member in &members {
        let member_id: String = member.get(0);
        let member_id_text = member_id.clone();
        let frame_head: i64 = member.get(1);
        let generation: i64 = member.get(2);
        let member_id = MemberId::from_str(&member_id)?;
        if witness.membership_generations().get(&member_id_text) != Some(&generation)
            || witness.observation_frame_heads().get(&member_id)
                != Some(&u64::try_from(frame_head)?)
        {
            bail!("captured witness does not match member generation or frame Head");
        }
    }
    if transition_rows != i64::try_from(SCALE)?
        || usize::try_from(timer_rows)? != witness.timers().len()
        || usize::try_from(membership_rows)? != witness.membership_generations().len()
        || members.len() != witness.observation_frame_heads().len()
        || observed_domains.len() != expected_operational_counts.len()
        || semantic_receipt_rows < transition_rows
    {
        bail!("captured witness does not exactly match transferred operational rows");
    }
    Ok(json!({
        "history_transition_rows": transition_rows,
        "head_room_seq": SCALE,
        "checkpoint_room_seq": checkpoint_seq,
        "checkpoint_tail_transition_count": candidate.tail_transition_count(),
        "recovery_execution_path": "checkpoint",
        "checkpoint_boundary_transition_records_read_by_adapter": 1,
        "prefix_transition_records_delivered_to_core": receipt.prefix_transition_records_delivered(),
        "tail_transition_records_delivered_to_core": receipt.tail_transition_records_delivered(),
        "prefix_transition_range_reads": 0,
        "prefix_transitions_skipped": receipt.prefix_transitions_skipped(),
        "transition_records_read_by_adapter_total": 1_u64
            .checked_add(receipt.tail_transition_records_delivered())
            .ok_or_else(|| anyhow!("scale Transition read count overflow"))?,
        "checkpoint_witness_bytes": witness_bytes.len(),
        "checkpoint_witness_under_16_mib": witness_bytes.len() <= 16 * 1024 * 1024,
        "reducer_callback_count": recovered.activity_callback_count(),
        "recovery_ms": recovery_ms,
        "rss_bytes": rss_bytes(),
        "state": {"head_exact": true, "core_exact": true, "activity_exact": true, "checkpoint_hash_exact": true},
        "operational_witness": {
            "timers": {"live_rows": timer_rows, "witness_entries": witness.timers().len(), "exact": true},
            "frames": {"live_rows": frame_rows, "witness_entries": witness.operational_history_roots()["frames"].entry_count(), "exact": true},
            "consequences": {"live_rows": consequence_rows, "witness_entries": witness.operational_history_roots()["consequences"].entry_count(), "exact": true},
            "membership_generations": {"live_rows": members.len(), "witness_entries": witness.membership_generations().len(), "exact": true},
            "frame_heads": {"live_rows": members.len(), "witness_entries": witness.observation_frame_heads().len(), "exact": true},
            "activation_decisions": {"live_rows": activation_rows, "witness_entries": witness.operational_history_roots()["activation_decisions"].entry_count(), "exact": true}
        },
        "semantic_receipts": {"live_rows": semantic_receipt_rows, "read_by_bounded_recovery": false},
        "qualification_setup": "public_sqlite_to_postgres_v2_transfer+full_verified_replay_checkpoint_capture"
    }))
}

fn main() -> Result<()> {
    let arguments = arguments()?;
    let output = arguments.output.clone();
    let report = run(arguments)?;
    publish(&output, &report)?;
    println!(
        "LIVE_POSTGRES_RECOVERY_SCALE={}",
        serde_json::to_string(&report)?
    );
    Ok(())
}
