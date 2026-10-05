//! Shared operational byte readers. Streaming verification discards payload bytes after checking their hashes.
use super::*;
pub(super) struct PostgresOperationalEvidence {
    pub membership_bytes: Vec<(String, Vec<u8>)>,
    pub observation_positions: Vec<PostgresObservationPositionEvidenceV1>,
    pub timers: Vec<PostgresTimerEvidenceV1>,
    pub frames: Vec<PostgresFrameEvidenceV1>,
    pub observation_consequences: Vec<PostgresObservationConsequenceEvidenceV1>,
    pub activation_decisions: Vec<PostgresActivationDecisionEvidenceV1>,
}
#[allow(
    clippy::too_many_lines,
    reason = "One provider capture retains the existing strict operational row readers."
)]
pub(super) fn capture_operational_evidence<C: GenericClient>(
    client: &mut C,
    room_id: &str,
    retain_payloads: bool,
) -> Result<PostgresOperationalEvidence, PostgresRoomVerificationError> {
    let member_rows = client
            .query(
                "SELECT member_id, membership_bytes, frame_head, retained_frame_floor, last_ack_frame_seq, reset_required_through, reset_generation FROM worldstream_members WHERE room_id = $1 ORDER BY member_id",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
    let mut canonical_membership_bytes = Vec::with_capacity(member_rows.len());
    let mut observation_positions = Vec::with_capacity(member_rows.len());
    for row in &member_rows {
        let member_id: String = row
            .try_get(0)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Membership" })?;
        let bytes: Vec<u8> = row
            .try_get(1)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Membership" })?;
        let membership = CanonicalJsonV1::decode_canonical::<MembershipV1>(&bytes)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Membership" })?;
        if membership.member_id().to_string() != member_id {
            return Err(PostgresRoomVerificationError::Corrupt { what: "Membership" });
        }
        let frame_head = u64::try_from(row.try_get::<_, i64>(2).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            }
        })?)
        .map_err(|_| PostgresRoomVerificationError::Corrupt {
            what: "observation position",
        })?;
        let retained_frame_floor = u64::try_from(row.try_get::<_, i64>(3).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            }
        })?)
        .map_err(|_| PostgresRoomVerificationError::Corrupt {
            what: "observation position",
        })?;
        if retained_frame_floor > frame_head.saturating_add(1) {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            });
        }
        let last_ack_frame_seq = row
            .try_get::<_, Option<i64>>(4)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            })?
            .map(|value| {
                u64::try_from(value).map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                })
            })
            .transpose()?;
        let reset_required_through = row
            .try_get::<_, Option<i64>>(5)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            })?
            .map(|value| {
                u64::try_from(value).map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation position",
                })
            })
            .transpose()?;
        let reset_generation = u64::try_from(row.try_get::<_, i64>(6).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            }
        })?)
        .map_err(|_| PostgresRoomVerificationError::Corrupt {
            what: "observation position",
        })?;
        if retained_frame_floor == 0
            || last_ack_frame_seq.is_some_and(|value| value == 0 || value > frame_head)
            || reset_required_through.is_some_and(|value| value > frame_head)
        {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "observation position",
            });
        }
        observation_positions.push(PostgresObservationPositionEvidenceV1 {
            member_id: member_id.clone(),
            frame_head,
            retained_frame_floor,
            last_ack_frame_seq,
            reset_required_through,
            reset_generation,
        });
        canonical_membership_bytes.push((member_id, bytes));
    }
    let timer_rows = client
            .query(
                "SELECT timer_id, generation, scheduled_for, payload_bytes, state FROM worldstream_timers WHERE room_id = $1 ORDER BY timer_id, generation",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
    let mut timers = Vec::with_capacity(timer_rows.len());
    for row in timer_rows {
        let generation: i64 = row
            .try_get(1)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?;
        timers.push(PostgresTimerEvidenceV1 {
            timer_id: row
                .try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
            generation: u64::try_from(generation)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
            scheduled_for: row
                .try_get(2)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
            payload_bytes: row
                .try_get(3)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
            state: row
                .try_get(4)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Timer" })?,
        });
    }
    let mut frame_rows = client
            .query_raw(
                "SELECT member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash FROM worldstream_frames WHERE room_id = $1 ORDER BY member_id, frame_seq",
                [&room_id as &(dyn postgres::types::ToSql + Sync)],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
    let mut frames = Vec::new();
    while let Some(row) = frame_rows
        .next()
        .map_err(PostgresRoomVerificationError::Sql)?
    {
        let frame_seq: i64 = row
            .try_get(1)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
        let cause_room_seq: i64 = row
            .try_get(2)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
        let member_id: String = row
            .try_get(0)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
        let payload_bytes: Vec<u8> = row
            .try_get(3)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
        let payload_hash: Vec<u8> = row
            .try_get(4)
            .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?;
        if member_id.parse::<worldstream_core::MemberId>().is_err()
            || RoomSequenceV1::new(
                u64::try_from(cause_room_seq)
                    .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?,
            )
            .is_err()
            || payload_hash.as_slice() != Blake3DigestV1::hash(&payload_bytes).as_bytes()
        {
            return Err(PostgresRoomVerificationError::Corrupt { what: "Frame" });
        }
        frames.push(PostgresFrameEvidenceV1 {
            member_id,
            frame_seq: u64::try_from(frame_seq)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?,
            cause_room_seq: u64::try_from(cause_room_seq)
                .map_err(|_| PostgresRoomVerificationError::Corrupt { what: "Frame" })?,
            payload_bytes: if retain_payloads {
                payload_bytes
            } else {
                Vec::new()
            },
            payload_hash,
        });
    }
    drop(frame_rows);
    let mut consequence_rows = client
            .query_raw(
                "SELECT member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash FROM worldstream_observation_consequences WHERE room_id = $1 ORDER BY member_id, cause_room_seq",
                [&room_id as &(dyn postgres::types::ToSql + Sync)],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
    let mut observation_consequences = Vec::new();
    while let Some(row) = consequence_rows
        .next()
        .map_err(PostgresRoomVerificationError::Sql)?
    {
        let member_id: String =
            row.try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation consequence",
                })?;
        let cause_room_seq = u64::try_from(row.try_get::<_, i64>(1).map_err(|_| {
            PostgresRoomVerificationError::Corrupt {
                what: "observation consequence",
            }
        })?)
        .map_err(|_| PostgresRoomVerificationError::Corrupt {
            what: "observation consequence",
        })?;
        if member_id.parse::<worldstream_core::MemberId>().is_err()
            || RoomSequenceV1::new(cause_room_seq).is_err()
        {
            return Err(PostgresRoomVerificationError::Corrupt {
                what: "observation consequence",
            });
        }
        let kind: String = row
            .try_get(2)
            .map_err(|_| PostgresRoomVerificationError::Corrupt {
                what: "observation consequence",
            })?;
        let payload_bytes: Option<Vec<u8>> =
            row.try_get(3)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation consequence",
                })?;
        let projection_hash: Option<Vec<u8>> =
            row.try_get(4)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "observation consequence",
                })?;
        let consequence = match (kind.as_str(), payload_bytes, projection_hash) {
            ("reset_required", Some(payload_bytes), Some(projection_hash))
                if projection_hash_for_canonical_bytes(&payload_bytes)
                    .is_ok_and(|digest| projection_hash.as_slice() == digest.as_bytes()) =>
            {
                PostgresObservationConsequenceEvidenceV1::ResetRequired {
                    member_id,
                    cause_room_seq,
                    payload_bytes: if retain_payloads {
                        payload_bytes
                    } else {
                        Vec::new()
                    },
                    projection_hash,
                }
            }
            ("visibility_lost", None, None) => {
                PostgresObservationConsequenceEvidenceV1::VisibilityLost {
                    member_id,
                    cause_room_seq,
                }
            }
            _ => {
                return Err(PostgresRoomVerificationError::Corrupt {
                    what: "observation consequence",
                });
            }
        };
        observation_consequences.push(consequence);
    }
    drop(consequence_rows);
    let decision_rows = client
            .query(
                "SELECT cause_room_seq, decision_id, target_member_id, decision_bytes FROM worldstream_activation_decisions WHERE room_id = $1 ORDER BY cause_room_seq, decision_id",
                &[&room_id],
            )
            .map_err(PostgresRoomVerificationError::Sql)?;
    let mut activation_decisions = Vec::with_capacity(decision_rows.len());
    for row in decision_rows {
        let cause_room_seq: i64 =
            row.try_get(0)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Activation decision",
                })?;
        activation_decisions.push(PostgresActivationDecisionEvidenceV1 {
            cause_room_seq: u64::try_from(cause_room_seq).map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "Activation decision",
                }
            })?,
            decision_id: row
                .try_get(1)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Activation decision",
                })?,
            target_member_id: row.try_get(2).map_err(|_| {
                PostgresRoomVerificationError::Corrupt {
                    what: "Activation decision",
                }
            })?,
            decision_bytes: row
                .try_get(3)
                .map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Activation decision",
                })?,
        });
    }
    Ok(PostgresOperationalEvidence {
        membership_bytes: canonical_membership_bytes,
        observation_positions,
        timers,
        frames,
        observation_consequences,
        activation_decisions,
    })
}
pub(super) fn verify_replay(
    replay: &ReplayStorageVerificationV1,
    stored: &PostgresOperationalEvidence,
) -> Result<(), RoomRecoveryErrorV1> {
    verify_replayed_membership_evidence(replay, &stored.membership_bytes)?;
    verify_replayed_timer_evidence(replay, &stored.timers)?;
    verify_replayed_activation_decision_evidence(replay, &stored.activation_decisions)?;
    verify_replayed_position_evidence(replay, &stored.observation_positions)?;
    verify_replayed_frame_evidence(replay, &stored.observation_positions, &stored.frames)?;
    verify_replayed_nonframe_consequences(replay, &stored.observation_consequences)
}

pub(super) fn verify_structural(
    structure: &worldstream_core::CanonicalStructuralHistory,
    canonical_membership_bytes: &[(String, Vec<u8>)],
    timers: &[PostgresTimerEvidenceV1],
    activation_decisions: &[PostgresActivationDecisionEvidenceV1],
) -> Result<(), PostgresRoomVerificationError> {
    let expected_members = structure
        .core_state()
        .memberships()
        .values()
        .map(|member| {
            let bytes =
                serde_json::to_vec(member).map_err(|_| PostgresRoomVerificationError::Corrupt {
                    what: "Membership derivation",
                })?;
            Ok((
                member.member_id().to_string(),
                CanonicalJsonV1::parse(&bytes)
                    .and_then(|value| value.to_bytes())
                    .map_err(|_| PostgresRoomVerificationError::Corrupt {
                        what: "Membership derivation",
                    })?,
            ))
        })
        .collect::<Result<Vec<_>, PostgresRoomVerificationError>>()?;
    if canonical_membership_bytes != expected_members {
        return Err(PostgresRoomVerificationError::Corrupt {
            what: "Membership agreement",
        });
    }
    let expected_timers = structure
        .timer_materializations()
        .into_iter()
        .map(|timer| PostgresTimerEvidenceV1 {
            timer_id: timer.timer_id().to_string(),
            generation: timer.generation().get(),
            scheduled_for: timer.scheduled_for().to_string(),
            payload_bytes: timer.canonical_payload_bytes().to_vec(),
            state: recovered_postgres_timer_state(timer.state()).into(),
        })
        .collect::<Vec<_>>();
    if timers != expected_timers {
        return Err(PostgresRoomVerificationError::Corrupt {
            what: "Timer agreement",
        });
    }
    let mut derived_decisions = structure.activation_decisions().iter().collect::<Vec<_>>();
    derived_decisions.sort_by(|left, right| {
        (left.cause_room_seq(), left.decision_id())
            .cmp(&(right.cause_room_seq(), right.decision_id()))
    });
    if activation_decisions.len() != derived_decisions.len()
        || activation_decisions
            .iter()
            .zip(derived_decisions)
            .any(|(stored, derived)| {
                stored.cause_room_seq != derived.cause_room_seq().get()
                    || stored.decision_id != derived.decision_id()
                    || stored.target_member_id
                        != derived.target_member_id().map(ToString::to_string)
                    || stored.decision_bytes != derived.canonical_decision_bytes()
            })
    {
        return Err(PostgresRoomVerificationError::Corrupt {
            what: "Activation decision agreement",
        });
    }
    Ok(())
}

pub(super) fn verify_membership_generations<C: GenericClient>(
    client: &mut C,
    room_id: &str,
    expected: &BTreeMap<String, i64>,
) -> Result<(), PostgresRoomVerificationError> {
    let rows=client.query("SELECT member_id, membership_generation FROM worldstream_members WHERE room_id=$1 ORDER BY member_id",&[&room_id]).map_err(PostgresRoomVerificationError::Sql)?;
    let stored = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get::<_, String>(0)
                    .map_err(PostgresRoomVerificationError::Sql)?,
                row.try_get::<_, i64>(1)
                    .map_err(PostgresRoomVerificationError::Sql)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, PostgresRoomVerificationError>>()?;
    if &stored != expected {
        return Err(PostgresRoomVerificationError::Corrupt {
            what: "Membership generation agreement",
        });
    }
    Ok(())
}
