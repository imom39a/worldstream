-- A Run is retained once Genesis is observed even if its Lobby never starts.
-- Pre-Host cancellation remains a separate launch-request transition: this
-- operation requires fresh, exact Host evidence that the retained task has
-- not committed launch. Database time may select a due candidate, but it
-- never substitutes for that evidence.
alter table platform_store.launch_requests
  drop constraint launch_request_state;

alter table platform_store.launch_requests
  add constraint launch_request_state check (state in (
    'collecting_roster',
    'provisioning',
    'reconciling',
    'run_created',
    'cancelled',
    'expired',
    'failed_pre_genesis',
    'abandoned_prestart'
  ));

-- The existing immutability trigger owns the transition graph. Extend it
-- narrowly instead of disabling it for the abandonment write.
create or replace function platform_store.protect_launch_request_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE'
    or new.launch_request_id <> old.launch_request_id
    or new.creator_account_id <> old.creator_account_id
    or new.listing_revision_digest <> old.listing_revision_digest
    or new.idempotency_namespace <> old.idempotency_namespace
    or new.idempotency_key_digest <> old.idempotency_key_digest
    or new.canonical_launch_input <> old.canonical_launch_input
    or new.launch_input_digest <> old.launch_input_digest
    or new.canonicalizer_version <> old.canonicalizer_version
    or new.house_fill_choice <> old.house_fill_choice
    or new.creator_access_choice <> old.creator_access_choice
    or new.creator_seat_id is distinct from old.creator_seat_id
    or new.creator_participation_kind is distinct from old.creator_participation_kind
    or new.expires_at <> old.expires_at
    or new.created_at <> old.created_at
    or new.last_transition_at < old.last_transition_at
    or old.roster_frozen_at is not null and (
      new.roster_frozen_at is distinct from old.roster_frozen_at
      or new.frozen_roster is distinct from old.frozen_roster
      or new.frozen_roster_digest is distinct from old.frozen_roster_digest
      or new.frozen_room_setup_specification is distinct from old.frozen_room_setup_specification
      or new.frozen_room_setup_specification_digest is distinct from old.frozen_room_setup_specification_digest
      or new.host_installation_id is distinct from old.host_installation_id
      or new.room_setup_operation_id is distinct from old.room_setup_operation_id
    )
    or old.host_mutation_started_at is not null
       and new.host_mutation_started_at is distinct from old.host_mutation_started_at
  then
    raise exception using errcode = '23000', message = 'immutable_launch_request';
  end if;

  if new.state <> old.state and not (
    (old.state = 'collecting_roster' and new.state in (
      'provisioning', 'cancelled', 'expired', 'failed_pre_genesis'
    ))
    or
    (old.state = 'provisioning' and new.state in (
      'reconciling', 'run_created', 'cancelled', 'expired', 'failed_pre_genesis'
    ))
    or
    (old.state = 'reconciling' and new.state in (
      'provisioning', 'run_created', 'failed_pre_genesis'
    ))
    or
    (old.state = 'run_created' and new.state = 'abandoned_prestart')
  ) then
    raise exception using errcode = '23000', message = 'invalid_launch_transition';
  end if;

  return new;
end;
$$;

create table platform_store.prestart_abandonments (
  activity_run_id uuid primary key
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  launch_request_id uuid not null unique
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  abandonment_evidence_digest bytea not null,
  canonical_abandonment_evidence bytea not null,
  recorded_at timestamptz not null default clock_timestamp(),
  constraint prestart_abandonment_digest_shape
    check (octet_length(abandonment_evidence_digest) = 32),
  constraint prestart_abandonment_evidence_bounds
    check (octet_length(canonical_abandonment_evidence) between 2 and 8192)
);

alter table platform_store.prestart_abandonments enable row level security;

create trigger protect_prestart_abandonment_v1
before update or delete on platform_store.prestart_abandonments
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

-- The candidate is deliberately small and contains no Room or Membership
-- material. A caller must still ask the exact retained Host operation for the
-- authoritative not-launched proof before it can record any release.
create function platform_api.list_prestart_abandonment_candidates_v1(
  p_limit integer
)
returns table(
  activity_run_id uuid,
  launch_request_id uuid,
  listing_revision_digest text,
  host_installation_id text,
  room_setup_operation_id text
)
language sql
stable
security invoker
set search_path = ''
as $$
  select
    runs.activity_run_id,
    launches.launch_request_id,
    launches.listing_revision_digest,
    runs.host_installation_id,
    runs.room_setup_operation_id
  from platform_store.launch_requests launches
  join platform_store.activity_runs runs
    on runs.launch_request_id = launches.launch_request_id
  join platform_store.activity_listing_revisions listings
    on listings.listing_revision_digest = launches.listing_revision_digest
  where launches.state = 'run_created'
    and runs.initial_reconciliation_state = 'ready'
    and runs.genesis_observed_at
      + make_interval(secs => listings.pre_start_deadline_seconds)
      <= clock_timestamp()
    and not exists (
      select 1 from platform_store.prestart_abandonments abandoned
      where abandoned.activity_run_id = runs.activity_run_id
    )
    and not exists (
      select 1 from platform_store.activity_run_terminal_evidence terminal
      where terminal.activity_run_id = runs.activity_run_id
    )
  order by runs.genesis_observed_at, runs.activity_run_id
  limit least(greatest(coalesce(p_limit, 0), 0), 100);
$$;

create function platform_api.record_prestart_abandonment_v1(
  p_launch_request_id uuid,
  p_canonical_abandonment_evidence bytea,
  p_abandonment_evidence_digest bytea
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  run_row platform_store.activity_runs%rowtype;
  existing platform_store.prestart_abandonments%rowtype;
  evidence jsonb;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_launch_request_id is null
    or octet_length(p_canonical_abandonment_evidence) not between 2 and 8192
    or octet_length(p_abandonment_evidence_digest) <> 32
    or p_abandonment_evidence_digest
      <> extensions.digest(p_canonical_abandonment_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_prestart_abandonment';
  end if;
  begin
    evidence := convert_from(p_canonical_abandonment_evidence, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_prestart_abandonment';
  end;
  if jsonb_typeof(evidence) <> 'object'
    or not (evidence ?& array[
      'schema', 'host_installation_id', 'launch_request_id',
      'listing_revision_digest', 'launch_request_digest',
      'room_setup_operation_id', 'room_id', 'lobby_launch_committed',
      'abandonment_fence_digest', 'authentication_tag'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema', 'host_installation_id', 'launch_request_id',
        'listing_revision_digest', 'launch_request_digest',
        'room_setup_operation_id', 'room_id', 'lobby_launch_committed',
        'abandonment_fence_digest', 'authentication_tag'
      )
    )
    or evidence ->> 'schema'
      <> 'worldstream/hosted-prestart-abandonment-evidence/v1'
    or evidence ->> 'host_installation_id'
      !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or evidence ->> 'launch_request_id'
      !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
    or evidence ->> 'listing_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'launch_request_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'room_setup_operation_id' !~ '^[a-z][a-z0-9-]{0,63}$'
    or evidence ->> 'room_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or evidence -> 'lobby_launch_committed' <> 'false'::jsonb
    or evidence ->> 'abandonment_fence_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'authentication_tag' !~ '^[0-9a-f]{64}$'
  then
    raise exception using errcode = '22023', message = 'invalid_prestart_abandonment';
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('prestart-abandonment:' || p_launch_request_id::text, 0)
  );
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then
    return false;
  end if;
  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.launch_request_id = p_launch_request_id;
  if not found then
    return false;
  end if;
  -- Share the terminal reconciler's Run lock. Whichever authoritative Host
  -- proof reaches this boundary first wins; a timing/deadline observation
  -- cannot race a terminal transition into releasing capacity twice.
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('run:' || run_row.activity_run_id::text, 0)
  );
  select abandoned.* into existing
  from platform_store.prestart_abandonments abandoned
  where abandoned.activity_run_id = run_row.activity_run_id;
  if found then
    return existing.canonical_abandonment_evidence = p_canonical_abandonment_evidence
      and existing.abandonment_evidence_digest = p_abandonment_evidence_digest;
  end if;

  -- Once terminal evidence or a committed Lobby exists, the Run wins the
  -- race. There is no elapsed-time fallback and no post-hoc state rewrite.
  if launch_row.state <> 'run_created'
    or run_row.initial_reconciliation_state <> 'ready'
    or exists (
      select 1 from platform_store.activity_run_terminal_evidence terminal
      where terminal.activity_run_id = run_row.activity_run_id
    )
  then
    return false;
  end if;
  -- The Host's retained launch digest is checked against the immutable Run
  -- correspondence, rather than recomputed from a browser payload.
  if evidence ->> 'host_installation_id' <> run_row.host_installation_id
    or evidence ->> 'launch_request_id' <> launch_row.launch_request_id::text
    or evidence ->> 'listing_revision_digest' <> launch_row.listing_revision_digest
    or evidence ->> 'launch_request_digest' <> run_row.launch_request_digest
    or evidence ->> 'room_setup_operation_id' <> run_row.room_setup_operation_id
    or evidence ->> 'room_id' <> run_row.room_id
  then
    return false;
  end if;

  insert into platform_store.prestart_abandonments (
    activity_run_id,
    launch_request_id,
    abandonment_evidence_digest,
    canonical_abandonment_evidence,
    recorded_at
  ) values (
    run_row.activity_run_id,
    launch_row.launch_request_id,
    p_abandonment_evidence_digest,
    p_canonical_abandonment_evidence,
    sampled_at
  );
  perform platform_store.release_launch_capacity_v1(
    p_launch_request_id,
    'abandoned_prestart'
  );
  update platform_store.launch_requests launches
  set state = 'abandoned_prestart', last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

revoke all on platform_store.prestart_abandonments
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.list_prestart_abandonment_candidates_v1(integer)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_prestart_abandonment_v1(uuid, bytea, bytea)
  from public, anon, authenticated, service_role;

grant select, insert on platform_store.prestart_abandonments to service_role;
grant execute on function platform_api.list_prestart_abandonment_candidates_v1(integer)
  to service_role;
grant execute on function platform_api.record_prestart_abandonment_v1(uuid, bytea, bytea)
  to service_role;
