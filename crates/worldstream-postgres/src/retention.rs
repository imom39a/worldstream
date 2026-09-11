//! Bounded, fair operational Observation retention; canonical history is untouched.

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use crate::{PostgresObservationError, PostgresRoomStore};

/// Runtime-owned maintenance progress. Each due pass visits at most eight
/// Memberships in stable order. Repeated passes eventually revisit every Room.
pub struct PostgresObservationRetentionV1 {
    after: Option<(String, String)>,
    next_due: Instant,
}

impl Default for PostgresObservationRetentionV1 {
    fn default() -> Self {
        Self {
            after: None,
            next_due: Instant::now(),
        }
    }
}

impl PostgresObservationRetentionV1 {
    /// Performs one bounded maintenance pass when due and returns changed Rooms
    /// so their live subscribers promptly observe the new reset fence.
    ///
    /// # Errors
    /// Returns a storage error without advancing any acknowledged Cursor.
    pub fn tick(
        &mut self,
        store: &PostgresRoomStore,
    ) -> Result<Vec<String>, PostgresObservationError> {
        if Instant::now() < self.next_due {
            return Ok(Vec::new());
        }
        self.next_due = Instant::now() + Duration::from_secs(5);
        let mut client = store
            .connect()
            .map_err(PostgresObservationError::Connection)?;
        let (after_room, after_member) = self
            .after
            .as_ref()
            .map_or(("", ""), |(room, member)| (room.as_str(), member.as_str()));
        let mut tx = client
            .transaction()
            .map_err(PostgresObservationError::Sql)?;
        tx.batch_execute("SET LOCAL statement_timeout = '250ms'; SET LOCAL lock_timeout = '25ms'")
            .map_err(PostgresObservationError::Sql)?;
        let rows = tx.query(
            "SELECT room_id, member_id FROM public.worldstream_members WHERE (room_id, member_id) > ($1, $2) ORDER BY room_id, member_id LIMIT 8",
            &[&after_room, &after_member],
        ).map_err(PostgresObservationError::Sql)?;
        tx.commit().map_err(PostgresObservationError::Sql)?;
        let mut changed = BTreeSet::new();
        for row in &rows {
            let room: String = row.try_get(0).map_err(PostgresObservationError::Sql)?;
            let member: String = row.try_get(1).map_err(PostgresObservationError::Sql)?;
            self.after = Some((room.clone(), member.clone()));
            match maintain_membership(&mut client, &room, &member) {
                Ok(deleted) if deleted > 0 => {
                    changed.insert(room);
                }
                Ok(_) => {}
                // A timed-out or damaged Room must not starve later Rooms.
                // The failed transaction rolls back; existing diagnostic
                // telemetry retains its closed storage failure classification.
                Err(error) => {
                    let _ = store.record_error(&error);
                }
            }
        }
        if rows.len() < 8 {
            self.after = None;
        }
        // Drain an existing backlog on following scheduler ticks instead of
        // capping a busy Membership at one 256-frame deletion every 5 seconds.
        if !changed.is_empty() {
            self.next_due = Instant::now();
        }
        Ok(changed.into_iter().collect())
    }
}

fn maintain_membership(
    client: &mut postgres::Client,
    room: &str,
    member: &str,
) -> Result<i64, postgres::Error> {
    let mut tx = client.transaction()?;
    tx.batch_execute("SET LOCAL statement_timeout = '250ms'; SET LOCAL lock_timeout = '25ms'")?;
    let deleted = tx
        .query_one(
            "SELECT public.worldstream_maintain_observation_retention_v1($1, $2)",
            &[&room, &member],
        )?
        .try_get(0)?;
    tx.commit()?;
    Ok(deleted)
}
