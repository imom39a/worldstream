//! Local, non-release evidence for a real large SQLite-to-PostgreSQL v2 stream.
//!
//! The companion shell runner owns a disposable PostgreSQL target. This
//! program only receives owner-readable DSN files and emits redacted JSON. It
//! deliberately uses the public manifest-bearing source, server import, and
//! authority-coordination seams: no `TransferBundleV1`, `BackupImageV1`, or
//! complete source record collection is constructed here.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Seek as _, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, anyhow, bail};
use postgres::{Client, NoTls};
use serde::Serialize;
use worldstream_backup::native_sqlite::{
    NativeSqliteStreamingLimitsV2, restore_file_streaming_v2, verify_retained_file_streaming_v2,
};
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresStreamDestinationV2,
    postgres_backend_fingerprint,
};
use worldstream_server::operator_transfer::{
    export_pending_stream_v2, finalize_stream_authority_v2, import_stream_chunks_v2,
};
use worldstream_sqlite::{
    SqliteRoomStore, SqliteSourceTransferStateV1, SqliteTransferStreamSourceV2,
};
use worldstream_transfer::{
    BackendFingerprintV1, BundleProfileV1, DigestV1, RecordParityV1, TargetFingerprintV1,
    TransferStreamDestinationV2, TransferStreamIdentityV2, TransferStreamLimitsV2,
    TransferStreamReaderV2, export_stream_v2,
};

const SCHEMA: &str = "worldstream/imo-225-local-stream-closure-evidence/v1";
const LARGE_TRANSITION_MINIMUM: u64 = 100_000;
const LARGE_BYTE_MINIMUM: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
struct Arguments {
    source: PathBuf,
    backup: PathBuf,
    restored_backup: PathBuf,
    stream: PathBuf,
    corrupted_stream: PathBuf,
    output: PathBuf,
    admin_dsn_file: PathBuf,
    corrupt_admin_dsn_file: PathBuf,
    stream_id: String,
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    status: &'static str,
    release_evidence: bool,
    source: SourceReport,
    stream: StreamReport,
    destination: DestinationReport,
    safety: SafetyReport,
    bounded_working_set: BoundedWorkingSet,
}

#[derive(Serialize)]
struct SourceReport {
    backup_streaming_verifier: &'static str,
    backup_streaming_restore: &'static str,
    restored_backup_page_count: u64,
    source_epoch: u64,
    transition_minimum: u64,
    canonical_record_count: u64,
    native_operational_record_count: u64,
    operational_relation_counts: BTreeMap<String, u64>,
    backup_digest: String,
}

#[derive(Serialize)]
struct StreamReport {
    expected_record_count: u64,
    expected_record_bytes: u64,
    stream_file_bytes: u64,
    chunk_count: u64,
    max_chunk_records_observed: usize,
    max_chunk_encoded_bytes_observed: u64,
    max_records_per_chunk_limit: usize,
    max_chunk_bytes_limit: usize,
    transition_threshold_passed: bool,
    byte_threshold_passed: bool,
}

#[derive(Serialize)]
struct DestinationReport {
    partial_checkpoint_next_chunk: u64,
    resume_import_finalized: bool,
    final_authority_published: bool,
    source_retired: bool,
    target_stream_state: Option<String>,
    hydrated_room_count: i64,
    hydrated_transition_count: i64,
    staged_stream_record_count: i64,
}

#[derive(Serialize)]
struct SafetyReport {
    disk_full_export_rejected: bool,
    disk_full_left_source_pending: bool,
    corrupted_stream_rejected: bool,
    corrupted_stream_hydrated_room_count: i64,
    corrupted_stream_published_authority: bool,
}

#[derive(Serialize)]
struct BoundedWorkingSet {
    source_rss_before_bytes: Option<u64>,
    source_rss_after_export_bytes: Option<u64>,
    source_rss_after_resume_import_bytes: Option<u64>,
    source_rss_after_corruption_probe_bytes: Option<u64>,
    source_rss_after_authority_bytes: Option<u64>,
    source_rss_peak_delta_bytes: Option<u64>,
    source_record_collection: &'static str,
    backup_image: &'static str,
}

#[derive(Default)]
struct TargetCounts {
    stream_state: Option<String>,
    hydrated_rooms: i64,
    hydrated_transitions: i64,
    staged_records: i64,
    authoritative: bool,
}

struct DiskFullWriter {
    remaining: usize,
}

impl Write for DiskFullWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        let written = bytes.len().min(self.remaining);
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn argument_value(arguments: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    arguments
        .next()
        .ok_or_else(|| anyhow!("missing value for {name}"))
}

fn arguments() -> Result<Arguments> {
    let mut values = env::args().skip(1);
    let mut source = None;
    let mut backup = None;
    let mut restored_backup = None;
    let mut stream = None;
    let mut corrupted_stream = None;
    let mut output = None;
    let mut admin_dsn_file = None;
    let mut corrupt_admin_dsn_file = None;
    let mut stream_id = None;
    while let Some(name) = values.next() {
        let value = match name.as_str() {
            "--source"
            | "--backup"
            | "--restored-backup"
            | "--stream"
            | "--corrupted-stream"
            | "--output"
            | "--admin-dsn-file"
            | "--corrupt-admin-dsn-file"
            | "--stream-id" => argument_value(&mut values, &name)?,
            _ => bail!("unknown argument {name}"),
        };
        match name.as_str() {
            "--source" => source = Some(PathBuf::from(value)),
            "--backup" => backup = Some(PathBuf::from(value)),
            "--restored-backup" => restored_backup = Some(PathBuf::from(value)),
            "--stream" => stream = Some(PathBuf::from(value)),
            "--corrupted-stream" => corrupted_stream = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--admin-dsn-file" => admin_dsn_file = Some(PathBuf::from(value)),
            "--corrupt-admin-dsn-file" => corrupt_admin_dsn_file = Some(PathBuf::from(value)),
            "--stream-id" => stream_id = Some(value),
            _ => unreachable!("recognized argument"),
        }
    }
    Ok(Arguments {
        source: source.ok_or_else(|| anyhow!("missing --source"))?,
        backup: backup.ok_or_else(|| anyhow!("missing --backup"))?,
        restored_backup: restored_backup.ok_or_else(|| anyhow!("missing --restored-backup"))?,
        stream: stream.ok_or_else(|| anyhow!("missing --stream"))?,
        corrupted_stream: corrupted_stream.ok_or_else(|| anyhow!("missing --corrupted-stream"))?,
        output: output.ok_or_else(|| anyhow!("missing --output"))?,
        admin_dsn_file: admin_dsn_file.ok_or_else(|| anyhow!("missing --admin-dsn-file"))?,
        corrupt_admin_dsn_file: corrupt_admin_dsn_file
            .ok_or_else(|| anyhow!("missing --corrupt-admin-dsn-file"))?,
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
        // The target commits exactly one authenticated chunk and its resume
        // cursor in a transaction. A 1k/4MiB cap keeps that working set
        // replaceable while avoiding thousands of round trips for a 100k
        // history with ordinary multi-kilobyte records.
        max_records_per_chunk: 1_000,
        max_chunk_bytes: 4 * 1024 * 1024,
        max_record_bytes: 64 * 1024,
        max_total_records: 2_000_000,
        max_total_record_bytes: 4 * 1024 * 1024 * 1024,
    }
}

fn target_for_stream(lineage: String, source_epoch: u64) -> Result<TargetFingerprintV1> {
    let target_epoch = source_epoch
        .checked_add(1)
        .ok_or_else(|| anyhow!("target epoch overflow"))?;
    Ok(TargetFingerprintV1::observed_canonical_export(
        target_epoch,
        lineage,
        postgres_backend_fingerprint()?,
        Vec::new(),
        DigestV1::hash(b"imo-225-local-stream-target-fence-v1"),
        RecordParityV1::from_records(&[])?,
    )?)
}

fn stream_identity(
    stream_id: &str,
    lineage: String,
    source_epoch: u64,
) -> Result<TransferStreamIdentityV2> {
    Ok(TransferStreamIdentityV2::new(
        stream_id,
        lineage,
        source_epoch,
        BundleProfileV1::SqliteBundled,
        BundleProfileV1::PostgresPrimary17,
    )?)
}

fn source_backend() -> Result<BackendFingerprintV1> {
    let target = postgres_backend_fingerprint()?;
    Ok(BackendFingerprintV1::new(
        BundleProfileV1::SqliteBundled,
        "sqlite-bundled",
        target.schema().clone(),
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
    let kib = std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

fn inspect_stream(
    path: &Path,
    limits: TransferStreamLimitsV2,
) -> Result<(u64, u64, u64, usize, u64)> {
    let file = File::open(path).context("open completed stream")?;
    let mut reader = TransferStreamReaderV2::new(file, limits).context("open stream reader")?;
    let manifest = reader
        .manifest()
        .cloned()
        .ok_or_else(|| anyhow!("stream manifest absent"))?;
    let mut chunks = 0_u64;
    let mut max_records = 0_usize;
    let mut max_encoded_bytes = 0_u64;
    while let Some(chunk) = reader.next_chunk().context("read stream chunk")? {
        chunks = chunks
            .checked_add(1)
            .ok_or_else(|| anyhow!("chunk count overflow"))?;
        max_records = max_records.max(chunk.records().len());
        max_encoded_bytes = max_encoded_bytes.max(u64::try_from(chunk.canonical_bytes()?.len())?);
    }
    let footer = reader
        .footer()
        .ok_or_else(|| anyhow!("stream footer absent"))?;
    if footer.record_count() != manifest.expected_record_count()
        || footer.record_bytes() != manifest.expected_record_bytes()
        || footer.stream_digest() != manifest.expected_stream_digest()
    {
        bail!("stream footer differs from manifest");
    }
    Ok((
        manifest.expected_record_count(),
        manifest.expected_record_bytes(),
        chunks,
        max_records,
        max_encoded_bytes,
    ))
}

fn target_counts(dsn: &str) -> Result<TargetCounts> {
    let mut client = Client::connect(dsn, NoTls).context("connect target evidence reader")?;
    let stream_state = client
        .query_opt(
            "SELECT state FROM worldstream_transfer_stream_imports_v2 ORDER BY stream_header_digest LIMIT 1",
            &[],
        )?
        .map(|row| row.try_get::<_, String>(0))
        .transpose()?;
    let hydrated_rooms: i64 = client
        .query_one("SELECT count(*)::bigint FROM worldstream_room_roots", &[])?
        .try_get(0)?;
    let hydrated_transitions: i64 = client
        .query_one("SELECT count(*)::bigint FROM worldstream_transitions", &[])?
        .try_get(0)?;
    let staged_records: i64 = client
        .query_one(
            "SELECT count(*)::bigint FROM worldstream_transfer_stream_records_v2",
            &[],
        )?
        .try_get(0)?;
    let target_fence_cleared: bool = client
        .query_one(
            "SELECT NOT EXISTS (SELECT 1 FROM worldstream_transfer_target_fence WHERE fence_id = true)",
            &[],
        )?
        .try_get(0)?;
    let authoritative = stream_state.as_deref() == Some("authoritative") && target_fence_cleared;
    Ok(TargetCounts {
        stream_state,
        hydrated_rooms,
        hydrated_transitions,
        staged_records,
        // An empty fresh target also has no fence. Publication requires the
        // durable stream journal to say `authoritative` as well as this fence
        // being cleared; callers make that conjunction explicitly.
        authoritative,
    })
}

fn corrupt_copy(source: &Path, destination: &Path) -> Result<()> {
    fs::copy(source, destination).context("copy stream for corruption probe")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(destination, fs::Permissions::from_mode(0o600))?;
    }
    let length = fs::metadata(destination)?.len();
    if length < 128 {
        bail!("stream is too short for isolated corruption probe");
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)?;
    let position = length / 2;
    file.seek(SeekFrom::Start(position))?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte)?;
    byte[0] ^= 0x80;
    file.seek(SeekFrom::Start(position))?;
    file.write_all(&byte)?;
    file.sync_all()?;
    Ok(())
}

fn run(arguments: Arguments) -> Result<Report> {
    let limits = limits();
    let source_rss_before = rss_bytes();
    let source = SqliteRoomStore::open(&arguments.source).context("open source SQLite")?;
    if source.source_transfer_state() != SqliteSourceTransferStateV1::SourceAuthoritative {
        bail!("source must be authoritative before this evidence run");
    }
    let pending = source
        .begin_source_transfer(&arguments.backup)
        .context("begin verified source transfer")?;
    if pending.state() != SqliteSourceTransferStateV1::TransferPending {
        bail!("source did not enter transfer-pending");
    }
    let backup_path = pending
        .backup_path()
        .ok_or_else(|| anyhow!("source retained backup path absent"))?;
    let backup_digest = pending
        .backup_digest()
        .ok_or_else(|| anyhow!("source retained backup digest absent"))?;
    let source_epoch = pending
        .source_epoch()
        .ok_or_else(|| anyhow!("source transfer epoch absent"))?;
    let verifier_file = File::open(backup_path).context("open retained backup for verifier")?;
    let verifier = verify_retained_file_streaming_v2(
        backup_path,
        &verifier_file,
        NativeSqliteStreamingLimitsV2::default(),
    )
    .context("streaming retained backup verifier")?;
    if DigestV1::from_bytes(&verifier.transfer_point_digest())? != backup_digest {
        bail!("streaming verifier digest differs from source transfer fence");
    }
    let restored_backup = restore_file_streaming_v2(backup_path, &arguments.restored_backup)
        .context("streaming retained backup restore")?;
    if restored_backup.source_transfer_point_digest != verifier.transfer_point_digest()
        || restored_backup.destination_transfer_point_digest != verifier.transfer_point_digest()
    {
        bail!("streaming restore digest differs from retained source");
    }

    let lineage = source.deployment_lineage().context("read source lineage")?;
    let identity = stream_identity(&arguments.stream_id, lineage.clone(), source_epoch)?;
    let target = target_for_stream(lineage.clone(), source_epoch)?;

    // A short writer fails before a complete artifact exists. It advances only
    // a source cursor in this process; the durable source authority remains
    // transfer-pending and no PostgreSQL destination has been opened.
    let source_backend = source_backend()?;
    let mut disk_full_source = SqliteTransferStreamSourceV2::open(
        backup_path,
        backup_digest,
        &identity,
        source_backend,
        postgres_backend_fingerprint()?,
    )?;
    let disk_full_export_rejected = export_stream_v2(
        DiskFullWriter { remaining: 1024 },
        identity.clone(),
        &mut disk_full_source,
        limits,
    )
    .is_err();
    let disk_full_left_source_pending =
        source.source_transfer_state() == SqliteSourceTransferStateV1::TransferPending;
    if !disk_full_export_rejected || !disk_full_left_source_pending {
        bail!("disk-full export safety contract was not observed");
    }

    let exported =
        export_pending_stream_v2(&source, &arguments.stream, &arguments.stream_id, limits)
            .context("export pending source stream")?;
    let source_rss_after_export = rss_bytes();
    let (expected_records, expected_bytes, chunk_count, max_chunk_records, max_chunk_bytes) =
        inspect_stream(&arguments.stream, limits)?;
    if expected_records != exported.record_count || expected_bytes != exported.record_bytes {
        bail!("export receipt differs from stream manifest");
    }

    let admin_dsn = read_dsn(&arguments.admin_dsn_file)?;
    let admin = PostgresAdmin::new(PostgresConnectionConfig::direct_admin(admin_dsn.clone())?)?;
    admin
        .migrate()
        .context("migrate primary PostgreSQL target")?;

    // Persist exactly one bounded chunk, abandon the process-local reader and
    // destination, then resume through the public server import wrapper.
    let partial_checkpoint_next_chunk = {
        let file = File::open(&arguments.stream)?;
        let mut reader = TransferStreamReaderV2::new(file, limits)?;
        let manifest = reader
            .manifest()
            .cloned()
            .ok_or_else(|| anyhow!("partial stream manifest absent"))?;
        let mut destination = PostgresStreamDestinationV2::new_with_manifest(
            &admin,
            reader.identity().clone(),
            manifest,
            reader.stream_header_digest(),
            target.clone(),
        )?;
        let first = reader
            .next_chunk()?
            .ok_or_else(|| anyhow!("stream did not contain a first chunk"))?;
        destination
            .apply_stream_chunk(reader.identity(), &first)?
            .next_chunk
    };
    if partial_checkpoint_next_chunk == 0 {
        bail!("interruption probe did not persist a checkpoint");
    }
    import_stream_chunks_v2(&arguments.stream, &admin, target.clone(), limits)
        .context("resume and semantically finalize v2 stream")?;
    let finalized_before_authority = target_counts(&admin_dsn)?;
    let resume_import_finalized = finalized_before_authority.stream_state.as_deref()
        == Some("finalized")
        && !finalized_before_authority.authoritative
        && finalized_before_authority.hydrated_rooms > 0;
    if !resume_import_finalized {
        bail!("resumed stream import did not remain finalized behind serving fence");
    }
    let source_rss_after_resume_import = rss_bytes();

    // A separate disposable target accepts the corrupted prefix only as
    // non-serving staging and must never hydrate or publish authority.
    corrupt_copy(&arguments.stream, &arguments.corrupted_stream)?;
    let corrupt_admin_dsn = read_dsn(&arguments.corrupt_admin_dsn_file)?;
    let corrupt_admin = PostgresAdmin::new(PostgresConnectionConfig::direct_admin(
        corrupt_admin_dsn.clone(),
    )?)?;
    corrupt_admin
        .migrate()
        .context("migrate corruption PostgreSQL target")?;
    let corrupted_stream_rejected = import_stream_chunks_v2(
        &arguments.corrupted_stream,
        &corrupt_admin,
        target.clone(),
        limits,
    )
    .is_err();
    let corrupt_counts = target_counts(&corrupt_admin_dsn)?;
    let corrupted_stream_hydrated_room_count = corrupt_counts.hydrated_rooms;
    let corrupted_stream_published_authority = corrupt_counts.authoritative;
    if !corrupted_stream_rejected
        || corrupted_stream_hydrated_room_count != 0
        || corrupted_stream_published_authority
    {
        bail!("corrupted stream crossed a target publication fence");
    }
    let source_rss_after_corruption_probe = rss_bytes();

    finalize_stream_authority_v2(&source, &arguments.stream, &admin, target, limits)
        .context("final stream authority coordinator")?;
    let final_target = target_counts(&admin_dsn)?;
    let source_retired =
        source.source_transfer_state() == SqliteSourceTransferStateV1::SourceRetired;
    let final_authority_published =
        final_target.stream_state.as_deref() == Some("authoritative") && final_target.authoritative;
    if !source_retired || !final_authority_published {
        bail!("authority handoff did not retire source before target publication");
    }
    let source_rss_after_authority = rss_bytes();
    let source_rss_peak_delta_bytes = match (
        source_rss_before,
        source_rss_after_export,
        source_rss_after_resume_import,
        source_rss_after_corruption_probe,
        source_rss_after_authority,
    ) {
        (
            Some(before),
            Some(after_export),
            Some(after_resume_import),
            Some(after_corruption_probe),
            Some(after_authority),
        ) => Some(
            after_export
                .max(after_resume_import)
                .max(after_corruption_probe)
                .max(after_authority)
                .saturating_sub(before),
        ),
        _ => None,
    };
    let expectations = {
        let file = File::open(&arguments.stream)?;
        let reader = TransferStreamReaderV2::new(file, limits)?;
        reader
            .manifest()
            .cloned()
            .ok_or_else(|| anyhow!("final stream manifest absent"))?
            .semantic_expectations()
            .to_owned()
    };
    let transition_threshold_passed =
        expectations.canonical_record_count() > LARGE_TRANSITION_MINIMUM;
    let byte_threshold_passed = expected_bytes > LARGE_BYTE_MINIMUM;
    if !transition_threshold_passed || !byte_threshold_passed {
        bail!("source did not exceed the required large-stream thresholds");
    }
    if final_target.hydrated_transitions < i64::try_from(LARGE_TRANSITION_MINIMUM + 1)? {
        bail!("target did not hydrate the full large transition history");
    }

    Ok(Report {
        schema: SCHEMA,
        status: "pass",
        release_evidence: false,
        source: SourceReport {
            backup_streaming_verifier: "verified_retained_file_streaming_v2",
            backup_streaming_restore: "restore_file_streaming_v2",
            restored_backup_page_count: restored_backup.page_count,
            source_epoch,
            transition_minimum: LARGE_TRANSITION_MINIMUM,
            canonical_record_count: expectations.canonical_record_count(),
            native_operational_record_count: expectations.native_operational_record_count(),
            operational_relation_counts: verifier.operational_relation_counts().clone(),
            backup_digest: backup_digest.to_string(),
        },
        stream: StreamReport {
            expected_record_count: expected_records,
            expected_record_bytes: expected_bytes,
            stream_file_bytes: fs::metadata(&arguments.stream)?.len(),
            chunk_count,
            max_chunk_records_observed: max_chunk_records,
            max_chunk_encoded_bytes_observed: max_chunk_bytes,
            max_records_per_chunk_limit: limits.max_records_per_chunk,
            max_chunk_bytes_limit: limits.max_chunk_bytes,
            transition_threshold_passed,
            byte_threshold_passed,
        },
        destination: DestinationReport {
            partial_checkpoint_next_chunk,
            resume_import_finalized,
            final_authority_published,
            source_retired,
            target_stream_state: final_target.stream_state,
            hydrated_room_count: final_target.hydrated_rooms,
            hydrated_transition_count: final_target.hydrated_transitions,
            staged_stream_record_count: final_target.staged_records,
        },
        safety: SafetyReport {
            disk_full_export_rejected,
            disk_full_left_source_pending,
            corrupted_stream_rejected,
            corrupted_stream_hydrated_room_count,
            corrupted_stream_published_authority,
        },
        bounded_working_set: BoundedWorkingSet {
            source_rss_before_bytes: source_rss_before,
            source_rss_after_export_bytes: source_rss_after_export,
            source_rss_after_resume_import_bytes: source_rss_after_resume_import,
            source_rss_after_corruption_probe_bytes: source_rss_after_corruption_probe,
            source_rss_after_authority_bytes: source_rss_after_authority,
            source_rss_peak_delta_bytes,
            source_record_collection: "keyset_cursor_and_bounded_chunks_only",
            backup_image: "not_constructed",
        },
    })
}

fn publish_report(path: &Path, report: &Report) -> Result<()> {
    if path.exists() {
        bail!("output path already exists");
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn main() -> Result<()> {
    let arguments = arguments()?;
    let output = arguments.output.clone();
    let report = run(arguments)?;
    publish_report(&output, &report)?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
