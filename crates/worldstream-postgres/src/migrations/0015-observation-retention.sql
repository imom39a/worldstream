-- Operational write time is outside canonical history. Existing rows receive
-- migration time because their original physical retention time is unknown.
ALTER TABLE public.worldstream_frames ADD COLUMN retained_at text NOT NULL
    DEFAULT to_char(statement_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"');

-- Runtime roles deliberately have no general DELETE privilege. This narrowly
-- scoped function permits only policy-expired observation prefixes to be
-- removed, after checking the caller already has runtime write authority.
CREATE FUNCTION public.worldstream_maintain_observation_retention_v1(
    target_room text, target_member text
) RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    position record;
    frame record;
    retained_bytes bigint := 0;
    retained_count integer := 0;
    desired_floor bigint;
    deleted_through bigint;
    deleted_count bigint := 0;
    needs_reset boolean;
BEGIN
    IF NOT has_table_privilege(session_user, 'public.worldstream_frames', 'INSERT')
       OR NOT has_table_privilege(session_user, 'public.worldstream_members', 'UPDATE') THEN
        RAISE EXCEPTION 'observation maintenance requires runtime write privileges'
            USING ERRCODE = '42501';
    END IF;

    -- The same root-before-member order as canonical commits; busy Rooms can
    -- wait for the next fair maintenance pass instead of delaying Actions.
    PERFORM room_id FROM public.worldstream_room_roots
        WHERE room_id = target_room AND integrity_status = 'healthy'
        FOR UPDATE SKIP LOCKED;
    IF NOT FOUND THEN RETURN 0; END IF;
    SELECT frame_head, retained_frame_floor, last_ack_frame_seq,
           reset_required_through, reset_generation INTO position
        FROM public.worldstream_members
        WHERE room_id = target_room AND member_id = target_member
        FOR UPDATE SKIP LOCKED;
    IF NOT FOUND THEN RETURN 0; END IF;

    desired_floor := greatest(position.retained_frame_floor, position.frame_head - 9999);
    -- Indexed metadata only. No payload is fetched or decompressed, and at
    -- most 10,001 rows are visited even after years without maintenance.
    FOR frame IN
        SELECT frame_seq, octet_length(payload_bytes) AS payload_size, retained_at
        FROM public.worldstream_frames
        WHERE room_id = target_room AND member_id = target_member
        ORDER BY frame_seq DESC LIMIT 10001
    LOOP
        retained_count := retained_count + 1;
        retained_bytes := retained_bytes + frame.payload_size;
        IF retained_count > 10000 OR retained_bytes > 67108864
           OR (frame.frame_seq <= coalesce(position.last_ack_frame_seq, 0)
               AND frame.retained_at::timestamptz <= statement_timestamp() - interval '7 days') THEN
            desired_floor := greatest(desired_floor, frame.frame_seq + 1);
            EXIT;
        END IF;
    END LOOP;
    IF desired_floor <= position.retained_frame_floor THEN RETURN 0; END IF;

    -- Repair a pre-existing oversized prefix gradually. Each transaction
    -- removes at most 256 frames and advances the floor only past that prefix.
    SELECT max(frame_seq) INTO deleted_through FROM (
        SELECT frame_seq FROM public.worldstream_frames
        WHERE room_id = target_room AND member_id = target_member
          AND frame_seq < desired_floor
        ORDER BY frame_seq LIMIT 256
    ) AS prefix;
    IF deleted_through IS NULL THEN RETURN 0; END IF;
    DELETE FROM public.worldstream_frames
        WHERE room_id = target_room AND member_id = target_member
          AND frame_seq <= deleted_through;
    GET DIAGNOSTICS deleted_count = ROW_COUNT;

    needs_reset := coalesce(position.last_ack_frame_seq, 0) < deleted_through;
    UPDATE public.worldstream_members
        SET retained_frame_floor = greatest(retained_frame_floor, deleted_through + 1),
            reset_required_through = CASE WHEN needs_reset THEN frame_head ELSE reset_required_through END,
            reset_generation = CASE WHEN needs_reset AND reset_required_through IS DISTINCT FROM frame_head
                THEN reset_generation + 1 ELSE reset_generation END
        WHERE room_id = target_room AND member_id = target_member;
    RETURN deleted_count;
END;
$$;
