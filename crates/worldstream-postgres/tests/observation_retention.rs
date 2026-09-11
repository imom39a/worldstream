//! Run with the dedicated disposable PostgreSQL test lane, never a user database.

use postgres::{Client, NoTls};
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresConnectionPath,
    PostgresObservationRetentionV1, PostgresRoomStore,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn prune(client: &mut Client, room: &str) -> Result<i64, postgres::Error> {
    client
        .query_one(
            "SELECT public.worldstream_maintain_observation_retention_v1($1, 'member')",
            &[&room],
        )?
        .try_get(0)
}

#[test]
fn live_retention_bounds_age_count_bytes_and_preserves_canonical_history() -> TestResult {
    let (Ok(admin_dsn), Ok(runtime_dsn)) = (
        std::env::var("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN"),
        std::env::var("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN"),
    ) else {
        eprintln!("LIVE_POSTGRES_RETENTION=SKIP reason=dsn_unset");
        return Ok(());
    };
    PostgresAdmin::new(PostgresConnectionConfig::direct_admin(&admin_dsn)?)?.migrate()?;
    let mut admin = Client::connect(&admin_dsn, NoTls)?;
    let mut runtime = Client::connect(&runtime_dsn, NoTls)?;
    for (room, head) in [
        ("retention-count", 10_258_i64),
        ("retention-age", 2),
        ("retention-bytes", 65),
    ] {
        admin.execute("INSERT INTO worldstream_room_roots(room_id, head_bytes, integrity_generation, integrity_status) VALUES ($1, 'canonical-head', 1, 'healthy')", &[&room])?;
        admin.execute("INSERT INTO worldstream_members(room_id, member_id, membership_bytes, frame_head) VALUES ($1, 'member', 'membership', $2)", &[&room, &head])?;
        admin.execute("INSERT INTO worldstream_transitions(room_id, room_seq, transition_bytes) VALUES ($1, 1, 'canonical-transition')", &[&room])?;
    }
    admin.batch_execute("UPDATE worldstream_members SET last_ack_frame_seq = 1 WHERE room_id = 'retention-age';
        INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash) SELECT 'retention-count', 'member', n, n, '{}'::bytea, 'hash'::bytea FROM generate_series(1, 10258) AS n;
        INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash, retained_at) VALUES ('retention-age', 'member', 1, 1, '{}', 'hash', '2000-01-01T00:00:00Z');
        INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash, retained_at) VALUES ('retention-age', 'member', 2, 2, '{}', 'hash', '2000-01-01T00:00:00Z');
        INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash) SELECT 'retention-bytes', 'member', n, n, repeat('x', 1048576)::bytea, 'hash'::bytea FROM generate_series(1, 65) AS n;")?;

    // The runtime can invoke policy maintenance without receiving DELETE on
    // any table or any authority over canonical history.
    assert!(
        !runtime
            .query_one(
                "SELECT has_table_privilege(current_user, 'public.worldstream_frames', 'DELETE')",
                &[]
            )?
            .get::<_, bool>(0)
    );
    assert_eq!(prune(&mut runtime, "retention-count")?, 256);
    assert_eq!(prune(&mut runtime, "retention-count")?, 2);
    assert_eq!(prune(&mut runtime, "retention-count")?, 0);
    assert_eq!(prune(&mut runtime, "retention-age")?, 1);
    assert_eq!(prune(&mut runtime, "retention-bytes")?, 1);
    assert_eq!(prune(&mut runtime, "retention-bytes")?, 0);
    for (room, expected_count, expected_floor, expected_epoch) in [
        ("retention-count", 10_000_i64, 259_i64, 1_i64),
        ("retention-age", 1, 2, 0),
        ("retention-bytes", 64, 2, 1),
    ] {
        assert_eq!(
            admin
                .query_one(
                    "SELECT count(*) FROM worldstream_frames WHERE room_id = $1",
                    &[&room]
                )?
                .get::<_, i64>(0),
            expected_count
        );
        let position = admin.query_one("SELECT retained_frame_floor, last_ack_frame_seq, reset_required_through, reset_generation, frame_head FROM worldstream_members WHERE room_id = $1", &[&room])?;
        assert_eq!(position.get::<_, i64>(0), expected_floor);
        assert_eq!(
            position.get::<_, Option<i64>>(1),
            if room == "retention-age" {
                Some(1)
            } else {
                None
            }
        );
        assert_eq!(
            position.get::<_, Option<i64>>(2),
            if room == "retention-age" {
                None
            } else {
                Some(position.get::<_, i64>(4))
            }
        );
        assert_eq!(position.get::<_, i64>(3), expected_epoch);
        assert_eq!(
            admin
                .query_one(
                    "SELECT transition_bytes FROM worldstream_transitions WHERE room_id = $1",
                    &[&room]
                )?
                .get::<_, Vec<u8>>(0),
            b"canonical-transition"
        );
        assert_eq!(
            admin
                .query_one(
                    "SELECT head_bytes FROM worldstream_room_roots WHERE room_id = $1",
                    &[&room]
                )?
                .get::<_, Vec<u8>>(0),
            b"canonical-head"
        );
    }

    // An old unacknowledged frame survives age pruning. Its later ACK starts
    // a fresh safety window, which duplicate ACKs cannot extend indefinitely.
    let store = PostgresRoomStore::new(PostgresConnectionConfig::runtime(
        &runtime_dsn,
        PostgresConnectionPath::Direct,
    )?)?;
    assert_eq!(prune(&mut runtime, "retention-age")?, 0);
    assert_eq!(
        store.acknowledge_observation("retention-age", "member", 2)?,
        Some(2)
    );
    let acknowledged_at: String = admin
        .query_one(
            "SELECT retained_at FROM worldstream_frames WHERE room_id = 'retention-age'",
            &[],
        )?
        .get(0);
    assert_ne!(acknowledged_at, "2000-01-01T00:00:00Z");
    assert_eq!(prune(&mut runtime, "retention-age")?, 0);
    assert_eq!(
        store.acknowledge_observation("retention-age", "member", 2)?,
        Some(2)
    );
    assert_eq!(
        admin
            .query_one(
                "SELECT retained_at FROM worldstream_frames WHERE room_id = 'retention-age'",
                &[]
            )?
            .get::<_, String>(0),
        acknowledged_at
    );

    // A busy Room never holds up maintenance on an unrelated Room.
    let mut lock = admin.transaction()?;
    lock.query_one(
        "SELECT room_id FROM worldstream_room_roots WHERE room_id = 'retention-age' FOR UPDATE",
        &[],
    )?;
    runtime.batch_execute("SET statement_timeout = '1s'")?;
    assert_eq!(prune(&mut runtime, "retention-age")?, 0);
    assert_eq!(prune(&mut runtime, "retention-count")?, 0);
    lock.rollback()?;

    // A backlog larger than one bounded page continues on the next scheduler
    // call without a five-second pause between every 256-frame deletion.
    admin.batch_execute("UPDATE worldstream_members SET frame_head = 10770 WHERE room_id = 'retention-count';
        INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash) SELECT 'retention-count', 'member', n, n, '{}'::bytea, 'hash'::bytea FROM generate_series(10259, 10770) AS n;")?;
    let mut maintenance = PostgresObservationRetentionV1::default();
    for expected_count in [10_256_i64, 10_000_i64] {
        assert!(
            maintenance
                .tick(&store)?
                .iter()
                .any(|room| room == "retention-count")
        );
        assert_eq!(
            admin
                .query_one(
                    "SELECT count(*) FROM worldstream_frames WHERE room_id = 'retention-count'",
                    &[]
                )?
                .get::<_, i64>(0),
            expected_count
        );
    }
    println!(
        "LIVE_POSTGRES_RETENTION=PASS age+count+bytes+bounded-delete+cursor+history+busy-room+backlog-drain"
    );
    Ok(())
}
