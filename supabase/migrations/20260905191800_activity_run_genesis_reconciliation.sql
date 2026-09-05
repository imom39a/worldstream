create table platform_store.activity_runs (
  activity_run_id uuid primary key default gen_random_uuid(),
  launch_request_id uuid not null unique
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  listing_revision_digest text not null
    references platform_store.activity_listing_revisions(listing_revision_digest)
    on delete restrict,
  creator_account_id uuid not null
    references platform_store.platform_accounts(account_id) on delete restrict,
  host_installation_id text not null,
  room_setup_operation_id text not null,
  room_id text not null,
  launch_request_digest text not null,
  pack_id text not null,
  pack_version text not null,
  pack_revision_digest text not null,
  genesis_room_seq bigint not null,
  genesis_or_transition_hash text not null,
  canonical_genesis_evidence bytea not null,
  genesis_evidence_digest bytea not null,
  public_id text,
  evidence_class text not null,
  initial_reconciliation_state text not null,
  genesis_observed_at timestamptz not null default clock_timestamp(),
  inserted_at timestamptz not null default clock_timestamp(),
  constraint activity_run_host_shape check (
    host_installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
  ),
  constraint activity_run_operation_shape check (
    room_setup_operation_id ~ '^[a-z][a-z0-9-]{0,63}$'
  ),
  constraint activity_run_room_shape check (
    room_id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
  ),
  constraint activity_run_launch_digest_shape check (
    launch_request_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint activity_run_pack_id_shape check (
    length(pack_id) between 1 and 128
    and pack_id ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
  ),
  constraint activity_run_pack_version_shape check (
    length(pack_version) between 1 and 64
    and pack_version !~ '[[:cntrl:]]'
  ),
  constraint activity_run_pack_digest_shape check (
    pack_revision_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint activity_run_genesis_sequence check (genesis_room_seq = 0),
  constraint activity_run_genesis_hash_shape check (
    genesis_or_transition_hash ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint activity_run_evidence_bounds check (
    octet_length(canonical_genesis_evidence) between 2 and 262144
  ),
  constraint activity_run_evidence_digest_sha256 check (
    octet_length(genesis_evidence_digest) = 32
  ),
  constraint activity_run_public_id_shape check (
    public_id is null or public_id ~ '^[0-9a-f]{32}$'
  ),
  constraint activity_run_evidence_class check (
    evidence_class in ('unranked', 'exhibition_platform_house_agents')
  ),
  constraint activity_run_initial_reconciliation check (
    initial_reconciliation_state in ('ready', 'quarantined')
  ),
  constraint activity_run_time_order check (inserted_at >= genesis_observed_at),
  unique (host_installation_id, room_id),
  unique (host_installation_id, room_setup_operation_id),
  unique (public_id)
);

create table platform_store.activity_run_memberships (
  activity_run_id uuid not null
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  membership_id text not null,
  access_mode text not null,
  purpose text not null,
  seat_id text,
  role text,
  public_seat_label text,
  principal_kind text not null,
  participation_source text,
  controlling_account_id uuid
    references platform_store.platform_accounts(account_id) on delete restrict,
  principal_id text not null,
  entry_selector text,
  service_scope_digest bytea,
  house_agent_assignment_id uuid
    references platform_store.house_agent_assignments(house_agent_assignment_id)
    on delete restrict,
  promoted_at timestamptz not null default clock_timestamp(),
  primary key (activity_run_id, membership_id),
  constraint activity_run_membership_id_shape check (
    membership_id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
  ),
  constraint activity_run_membership_access check (
    access_mode in ('participant', 'spectator')
  ),
  constraint activity_run_membership_purpose check (
    purpose in (
      'participant',
      'creator_spectator',
      'result_indexer',
      'public_projection_relay'
    )
  ),
  constraint activity_run_membership_seat_shape check (
    seat_id is null or seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'
  ),
  constraint activity_run_membership_role_shape check (
    role is null or role ~ '^[a-z][a-z0-9_-]{0,63}$'
  ),
  constraint activity_run_membership_public_label_shape check (
    public_seat_label is null
    or (
      length(public_seat_label) between 1 and 128
      and public_seat_label !~ '[[:cntrl:]]'
    )
  ),
  constraint activity_run_membership_principal_kind check (
    principal_kind in ('human', 'agent')
  ),
  constraint activity_run_membership_source check (
    participation_source is null
    or participation_source in (
      'account_human',
      'account_external_agent',
      'platform_house_agent'
    )
  ),
  constraint activity_run_membership_principal_id_shape check (
    principal_id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
  ),
  constraint activity_run_membership_selector_shape check (
    entry_selector is null or entry_selector ~ '^[0-9a-f]{32}$'
  ),
  constraint activity_run_membership_scope_digest check (
    service_scope_digest is null or octet_length(service_scope_digest) = 32
  ),
  constraint activity_run_membership_exact_shape check (
    (
      purpose = 'participant'
      and access_mode = 'participant'
      and seat_id is not null
      and role is not null
      and public_seat_label is not null
      and participation_source is not null
      and service_scope_digest is null
      and (
        (participation_source = 'account_human'
          and principal_kind = 'human'
          and controlling_account_id is not null
          and entry_selector is not null
          and house_agent_assignment_id is null)
        or
        (participation_source = 'account_external_agent'
          and principal_kind = 'agent'
          and controlling_account_id is not null
          and entry_selector is not null
          and house_agent_assignment_id is null)
        or
        (participation_source = 'platform_house_agent'
          and principal_kind = 'agent'
          and controlling_account_id is null
          and entry_selector is null
          and house_agent_assignment_id is not null)
      )
    )
    or
    (
      purpose = 'creator_spectator'
      and access_mode = 'spectator'
      and seat_id is null
      and role is null
      and public_seat_label is null
      and principal_kind = 'human'
      and participation_source = 'account_human'
      and controlling_account_id is not null
      and entry_selector is not null
      and service_scope_digest is not null
      and house_agent_assignment_id is null
    )
    or
    (
      purpose in ('result_indexer', 'public_projection_relay')
      and access_mode = 'spectator'
      and seat_id is null
      and role is null
      and public_seat_label is null
      and principal_kind = 'agent'
      and participation_source is null
      and controlling_account_id is null
      and entry_selector is null
      and service_scope_digest is not null
      and house_agent_assignment_id is null
    )
  ),
  unique (membership_id),
  unique (principal_id),
  unique (activity_run_id, seat_id),
  unique (activity_run_id, purpose, seat_id),
  unique (activity_run_id, entry_selector)
);

create table platform_store.reconciliation_receipts (
  reconciliation_receipt_id uuid primary key default gen_random_uuid(),
  receipt_kind text not null,
  source_key text not null,
  launch_request_id uuid
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  activity_run_id uuid
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  evidence_digest bytea not null,
  disposition text not null,
  safe_code text not null,
  observed_at timestamptz not null default clock_timestamp(),
  constraint reconciliation_receipt_kind check (
    receipt_kind in ('genesis', 'terminal', 'result', 'integrity')
  ),
  constraint reconciliation_receipt_source_bounds check (
    length(source_key) between 1 and 512
    and source_key !~ '[[:cntrl:]]'
  ),
  constraint reconciliation_receipt_evidence_sha256 check (
    octet_length(evidence_digest) = 32
  ),
  constraint reconciliation_receipt_disposition check (
    disposition in ('applied', 'duplicate', 'conflict', 'blocked', 'retryable')
  ),
  constraint reconciliation_receipt_code_shape check (
    safe_code ~ '^[a-z][a-z0-9_]{0,63}$'
  )
);

create index reconciliation_receipts_launch_idx
  on platform_store.reconciliation_receipts (
    launch_request_id,
    receipt_kind,
    observed_at desc,
    reconciliation_receipt_id
  );
create index reconciliation_receipts_run_idx
  on platform_store.reconciliation_receipts (
    activity_run_id,
    receipt_kind,
    observed_at desc,
    reconciliation_receipt_id
  ) where activity_run_id is not null;
create unique index reconciliation_receipts_idempotency_uq
  on platform_store.reconciliation_receipts (
    receipt_kind,
    launch_request_id,
    source_key,
    evidence_digest,
    disposition,
    safe_code
  ) where launch_request_id is not null;
create index activity_run_memberships_account_idx
  on platform_store.activity_run_memberships (
    controlling_account_id,
    activity_run_id,
    purpose
  ) where controlling_account_id is not null;

alter table platform_store.capacity_reservations
  add constraint capacity_reservation_activity_run_fk
  foreign key (activity_run_id)
  references platform_store.activity_runs(activity_run_id)
  on delete restrict;

alter table platform_store.activity_runs enable row level security;
alter table platform_store.activity_run_memberships enable row level security;
alter table platform_store.reconciliation_receipts enable row level security;

create function platform_store.reject_immutable_activity_run_row_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  raise exception using errcode = '23000', message = 'immutable_activity_run_row';
end;
$$;

create function platform_api.read_genesis_reconciliation_v1(
  p_launch_request_id uuid
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  run_row platform_store.activity_runs%rowtype;
  v_state text;
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id;
  if not found then
    return null;
  end if;

  select runs.*
  into run_row
  from platform_store.activity_runs runs
  where runs.launch_request_id = p_launch_request_id;

  if run_row.activity_run_id is null then
    v_state := case
      when exists (
        select 1
        from platform_store.reconciliation_receipts receipts
        where receipts.launch_request_id = p_launch_request_id
          and receipts.receipt_kind = 'genesis'
          and receipts.disposition = 'conflict'
      ) then 'quarantined'
      else 'pending'
    end;
  else
    v_state := case
      when run_row.initial_reconciliation_state = 'quarantined'
        or exists (
          select 1
          from platform_store.reconciliation_receipts receipts
          where receipts.activity_run_id = run_row.activity_run_id
            and receipts.disposition = 'conflict'
        )
      then 'quarantined'
      else 'ready'
    end;
  end if;

  return jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_genesis_reconciliation.v1',
    'launch_request_id', launch_row.launch_request_id,
    'launch_state', launch_row.state,
    'host_installation_id', launch_row.host_installation_id,
    'room_setup_operation_id', launch_row.room_setup_operation_id,
    'activity_run_id', run_row.activity_run_id,
    'genesis_recorded', run_row.activity_run_id is not null,
    'reconciliation_state', v_state,
    'needs_genesis_pull',
      launch_row.host_mutation_started_at is not null
      and run_row.activity_run_id is null
      and v_state = 'pending'
  ));
end;
$$;

create function platform_api.read_owned_run_v1(
  p_requesting_account_id uuid,
  p_activity_run_id uuid
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  run_row platform_store.activity_runs%rowtype;
  owned_memberships jsonb;
  v_state text;
begin
  if p_requesting_account_id is null or p_activity_run_id is null
    or not exists (
      select 1
      from platform_store.platform_accounts accounts
      where accounts.account_id = p_requesting_account_id
        and accounts.erased_at is null
    )
  then
    return null;
  end if;

  select runs.*
  into run_row
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  if not found or not exists (
    select 1
    from platform_store.activity_run_memberships memberships
    where memberships.activity_run_id = p_activity_run_id
      and memberships.controlling_account_id = p_requesting_account_id
  ) then
    return null;
  end if;

  v_state := case
    when run_row.initial_reconciliation_state = 'quarantined'
      or exists (
        select 1
        from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = p_activity_run_id
          and receipts.disposition = 'conflict'
      )
    then 'quarantined'
    else 'ready'
  end;

  select jsonb_agg(
    jsonb_strip_nulls(jsonb_build_object(
      'purpose', memberships.purpose,
      'seat_id', memberships.seat_id,
      'role', memberships.role,
      'public_seat_label', memberships.public_seat_label,
      'entry_selector', case
        when v_state = 'ready' then memberships.entry_selector
        else null
      end
    ))
    order by memberships.seat_id nulls last, memberships.purpose
  )
  into owned_memberships
  from platform_store.activity_run_memberships memberships
  where memberships.activity_run_id = p_activity_run_id
    and memberships.controlling_account_id = p_requesting_account_id;

  return jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_owned_run.v1',
    'run_id', run_row.activity_run_id,
    'listing_revision_digest', run_row.listing_revision_digest,
    'pack', jsonb_build_object(
      'id', run_row.pack_id,
      'version', run_row.pack_version,
      'digest', run_row.pack_revision_digest
    ),
    'public_id', run_row.public_id,
    'evidence_class', run_row.evidence_class,
    'reconciliation_state', v_state,
    'can_enter', v_state = 'ready',
    'memberships', coalesce(owned_memberships, '[]'::jsonb)
  ));
end;
$$;

create function platform_api.resolve_owned_run_membership_v1(
  p_requesting_account_id uuid,
  p_activity_run_id uuid,
  p_entry_selector text
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  resolved jsonb;
begin
  if p_requesting_account_id is null
    or p_activity_run_id is null
    or p_entry_selector !~ '^[0-9a-f]{32}$'
  then
    return null;
  end if;

  select jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_owned_run_membership.v1',
    'run_id', runs.activity_run_id,
    'listing_revision_digest', runs.listing_revision_digest,
    'host_installation_id', runs.host_installation_id,
    'room_setup_operation_id', runs.room_setup_operation_id,
    'room_id', runs.room_id,
    'pack', jsonb_build_object(
      'id', runs.pack_id,
      'version', runs.pack_version,
      'digest', runs.pack_revision_digest
    ),
    'client_release_digest', listings.client_release_digest,
    'client_surface_id', listings.client_surface_id,
    'access_mode', memberships.access_mode,
    'purpose', memberships.purpose,
    'seat_id', memberships.seat_id,
    'role', memberships.role,
    'principal_kind', memberships.principal_kind,
    'principal_id', memberships.principal_id,
    'membership_id', memberships.membership_id
  ))
  into resolved
  from platform_store.activity_runs runs
  join platform_store.activity_run_memberships memberships
    on memberships.activity_run_id = runs.activity_run_id
  join platform_store.activity_listing_revisions listings
    on listings.listing_revision_digest = runs.listing_revision_digest
  join platform_store.platform_accounts accounts
    on accounts.account_id = memberships.controlling_account_id
  where runs.activity_run_id = p_activity_run_id
    and runs.initial_reconciliation_state = 'ready'
    and memberships.controlling_account_id = p_requesting_account_id
    and memberships.entry_selector = p_entry_selector
    and accounts.erased_at is null
    and not exists (
      select 1
      from platform_store.reconciliation_receipts receipts
      where receipts.activity_run_id = runs.activity_run_id
        and receipts.disposition = 'conflict'
    );
  return resolved;
end;
$$;

revoke all on platform_store.activity_runs
  from public, anon, authenticated, service_role;
revoke all on platform_store.activity_run_memberships
  from public, anon, authenticated, service_role;
revoke all on platform_store.reconciliation_receipts
  from public, anon, authenticated, service_role;

revoke execute on function platform_store.reject_immutable_activity_run_row_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_genesis_reconciliation_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_owned_run_v1(uuid, uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.resolve_owned_run_membership_v1(uuid, uuid, text)
  from public, anon, authenticated, service_role;

grant select, insert on platform_store.activity_runs to service_role;
grant select, insert on platform_store.activity_run_memberships to service_role;
grant select, insert on platform_store.reconciliation_receipts to service_role;

grant execute on function platform_api.read_genesis_reconciliation_v1(uuid)
  to service_role;
grant execute on function platform_api.read_owned_run_v1(uuid, uuid)
  to service_role;
grant execute on function platform_api.resolve_owned_run_membership_v1(uuid, uuid, text)
  to service_role;

create trigger protect_activity_run_v1
before update or delete on platform_store.activity_runs
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create trigger protect_activity_run_membership_v1
before update or delete on platform_store.activity_run_memberships
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create trigger protect_reconciliation_receipt_v1
before update or delete on platform_store.reconciliation_receipts
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create unique index activity_run_memberships_spectator_purpose_uq
  on platform_store.activity_run_memberships (activity_run_id, purpose)
  where purpose <> 'participant';

create function platform_api.record_genesis_v1(
  p_launch_request_id uuid,
  p_canonical_genesis_evidence bytea,
  p_genesis_evidence_digest bytea
)
returns table(
  activity_run_id uuid,
  public_id text,
  reconciliation_state text,
  disposition text,
  safe_code text,
  evidence_class text
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  existing_run platform_store.activity_runs%rowtype;
  existing_membership platform_store.activity_run_memberships%rowtype;
  evidence jsonb;
  frozen_roster jsonb;
  frozen_setup jsonb;
  listing_document jsonb;
  member jsonb;
  derived_member jsonb;
  roster_member jsonb;
  listing_seat jsonb;
  derived_members jsonb := '[]'::jsonb;
  v_run_id uuid;
  conflicting_run_id uuid;
  v_public_id text;
  v_initial_state text;
  v_evidence_class text;
  v_reconciliation_state text;
  v_disposition text;
  v_safe_code text;
  v_source_key text;
  v_purpose text;
  v_setup_purpose text;
  v_source text;
  v_expected_kind text;
  v_controlling_account_id uuid;
  v_house_agent_assignment_id uuid;
  v_scope_digest bytea;
  expected_members integer;
  current_members integer;
  repaired_members integer := 0;
  identity_valid boolean := true;
  membership_valid boolean := true;
  authorization_valid boolean := false;
  correspondence_conflict boolean := false;
  authorized_capacity_present boolean := false;
  has_prior_conflict boolean := false;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_launch_request_id is null
    or octet_length(p_canonical_genesis_evidence) not between 2 and 262144
    or octet_length(p_genesis_evidence_digest) <> 32
    or p_genesis_evidence_digest
      <> extensions.digest(p_canonical_genesis_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_genesis_evidence';
  end if;

  begin
    evidence := convert_from(p_canonical_genesis_evidence, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_genesis_evidence';
  end;

  if jsonb_typeof(evidence) <> 'object'
    or evidence ->> 'schema' <> 'worldstream/hosted-genesis-evidence/v1'
    or not (evidence ?& array[
      'schema',
      'host_installation_id',
      'launch_request_id',
      'listing_revision_digest',
      'launch_request_digest',
      'frozen_roster_digest',
      'room_setup_specification_digest',
      'room_setup_operation_id',
      'room_id',
      'pack',
      'genesis_head',
      'memberships'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema',
        'host_installation_id',
        'launch_request_id',
        'listing_revision_digest',
        'launch_request_digest',
        'frozen_roster_digest',
        'room_setup_specification_digest',
        'room_setup_operation_id',
        'room_id',
        'pack',
        'genesis_head',
        'memberships'
      )
    )
    or (evidence ->> 'launch_request_id')
      !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    or (evidence ->> 'launch_request_id')::uuid <> p_launch_request_id
    or (evidence ->> 'host_installation_id')
      !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or (evidence ->> 'listing_revision_digest') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence ->> 'launch_request_digest') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence ->> 'frozen_roster_digest') !~ '^sha256:[0-9a-f]{64}$'
    or (evidence ->> 'room_setup_specification_digest')
      !~ '^blake3:[0-9a-f]{64}$'
    or (evidence ->> 'room_setup_operation_id') !~ '^[a-z][a-z0-9-]{0,63}$'
    or (evidence ->> 'room_id') !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or jsonb_typeof(evidence -> 'pack') <> 'object'
    or not ((evidence -> 'pack') ?& array['id', 'version', 'digest'])
    or exists (
      select 1 from jsonb_object_keys(evidence -> 'pack') keys(key)
      where keys.key not in ('id', 'version', 'digest')
    )
    or length(evidence #>> '{pack,id}') not between 1 and 128
    or (evidence #>> '{pack,id}') !~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    or length(evidence #>> '{pack,version}') not between 1 and 64
    or (evidence #>> '{pack,version}') ~ '[[:cntrl:]]'
    or (evidence #>> '{pack,digest}') !~ '^blake3:[0-9a-f]{64}$'
    or jsonb_typeof(evidence -> 'genesis_head') <> 'object'
    or not ((evidence -> 'genesis_head') ?& array[
      'room_id',
      'room_seq',
      'genesis_or_transition_hash',
      'core_schema_version',
      'pack_digest',
      'core_state_hash',
      'activity_state_hash',
      'authoritative_state_hash'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence -> 'genesis_head') keys(key)
      where keys.key not in (
        'room_id',
        'room_seq',
        'genesis_or_transition_hash',
        'core_schema_version',
        'pack_digest',
        'core_state_hash',
        'activity_state_hash',
        'authoritative_state_hash'
      )
    )
    or evidence #>> '{genesis_head,room_id}' <> evidence ->> 'room_id'
    or jsonb_typeof(evidence #> '{genesis_head,room_seq}') <> 'number'
    or evidence #>> '{genesis_head,room_seq}' <> '0'
    or length(evidence #>> '{genesis_head,core_schema_version}') not between 1 and 128
    or (evidence #>> '{genesis_head,core_schema_version}')
      !~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    or (evidence #>> '{genesis_head,genesis_or_transition_hash}')
      !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,pack_digest}') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,core_state_hash}') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,activity_state_hash}')
      !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,authoritative_state_hash}')
      !~ '^blake3:[0-9a-f]{64}$'
    or evidence #>> '{genesis_head,pack_digest}' <> evidence #>> '{pack,digest}'
    or jsonb_typeof(evidence -> 'memberships') <> 'array'
    or jsonb_array_length(evidence -> 'memberships') not between 1 and 35
    or exists (
      select 1
      from jsonb_array_elements(evidence -> 'memberships') members(value)
      where jsonb_typeof(members.value) <> 'object'
        or not (members.value ?& array[
          'access_mode',
          'purpose',
          'seat_id',
          'role',
          'principal_kind',
          'principal_id',
          'membership_id'
        ])
        or exists (
          select 1 from jsonb_object_keys(members.value) member_keys(key)
          where member_keys.key not in (
            'access_mode',
            'purpose',
            'seat_id',
            'role',
            'principal_kind',
            'principal_id',
            'membership_id',
            'scopes'
          )
        )
        or members.value ->> 'access_mode' not in ('participant', 'spectator')
        or members.value ->> 'purpose' not in (
          'participant',
          'creator_spectator',
          'result_indexer',
          'public_projection_relay'
        )
        or members.value ->> 'principal_kind' not in ('human', 'agent')
        or members.value ->> 'principal_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
        or members.value ->> 'membership_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    )
    or (
      select count(*) <> count(distinct value ->> 'principal_id')
      from jsonb_array_elements(evidence -> 'memberships')
    )
    or (
      select count(*) <> count(distinct value ->> 'membership_id')
      from jsonb_array_elements(evidence -> 'memberships')
    )
    or (
      select count(*) <> count(distinct value ->> 'seat_id')
      from jsonb_array_elements(evidence -> 'memberships')
      where value ->> 'purpose' = 'participant'
    )
    or (
      select count(*) <> count(distinct value ->> 'purpose')
      from jsonb_array_elements(evidence -> 'memberships')
      where value ->> 'purpose' <> 'participant'
    )
    or (
      select count(*) <> 1
      from jsonb_array_elements(evidence -> 'memberships')
      where value ->> 'purpose' = 'result_indexer'
    )
  then
    raise exception using errcode = '22023', message = 'invalid_genesis_evidence';
  end if;

  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then
    raise exception using errcode = '55000', message = 'launch_not_found';
  end if;

  select listings.*
  into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  begin
    frozen_roster := convert_from(launch_row.frozen_roster, 'utf8')::jsonb;
    frozen_setup := convert_from(launch_row.frozen_room_setup_specification, 'utf8')::jsonb;
    listing_document := convert_from(listing_row.canonical_document, 'utf8')::jsonb;
  exception when others then
    identity_valid := false;
    frozen_roster := jsonb_build_object('members', '[]'::jsonb);
    frozen_setup := jsonb_build_object('spectators', '[]'::jsonb);
    listing_document := '{}'::jsonb;
  end;

  v_source_key := 'host:' || (evidence ->> 'host_installation_id')
    || '/room:' || (evidence ->> 'room_id') || '/genesis:0';
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended(
      'host:' || (evidence ->> 'host_installation_id')
        || '/operation:' || (evidence ->> 'room_setup_operation_id'),
      0
    )
  );
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended(v_source_key, 0)
  );

  select runs.*
  into existing_run
  from platform_store.activity_runs runs
  where runs.launch_request_id = p_launch_request_id;
  if found then
    v_run_id := existing_run.activity_run_id;
    if existing_run.genesis_evidence_digest <> p_genesis_evidence_digest
      or existing_run.host_installation_id <> evidence ->> 'host_installation_id'
      or existing_run.room_setup_operation_id <> evidence ->> 'room_setup_operation_id'
      or existing_run.room_id <> evidence ->> 'room_id'
      or existing_run.listing_revision_digest <> launch_row.listing_revision_digest
      or existing_run.launch_request_digest <> evidence ->> 'launch_request_digest'
      or existing_run.pack_revision_digest <> evidence #>> '{pack,digest}'
      or existing_run.genesis_or_transition_hash
        <> evidence #>> '{genesis_head,genesis_or_transition_hash}'
    then
      insert into platform_store.reconciliation_receipts (
        receipt_kind,
        source_key,
        launch_request_id,
        activity_run_id,
        evidence_digest,
        disposition,
        safe_code,
        observed_at
      ) values (
        'genesis',
        v_source_key,
        p_launch_request_id,
        v_run_id,
        p_genesis_evidence_digest,
        'conflict',
        'genesis_identity_conflict',
        sampled_at
      ) on conflict do nothing;
      return query select
        v_run_id,
        existing_run.public_id,
        'quarantined'::text,
        'conflict'::text,
        'genesis_identity_conflict'::text,
        existing_run.evidence_class;
      return;
    end if;
    select exists (
      select 1
      from platform_store.reconciliation_receipts receipts
      where receipts.activity_run_id = v_run_id
        and receipts.disposition = 'conflict'
    ) into has_prior_conflict;
  else
    select runs.activity_run_id
    into conflicting_run_id
    from platform_store.activity_runs runs
    where (
        runs.host_installation_id = evidence ->> 'host_installation_id'
        and runs.room_id = evidence ->> 'room_id'
      )
      or (
        runs.host_installation_id = evidence ->> 'host_installation_id'
        and runs.room_setup_operation_id = evidence ->> 'room_setup_operation_id'
    )
    order by runs.activity_run_id
    limit 1;
    if found then
      insert into platform_store.reconciliation_receipts (
        receipt_kind,
        source_key,
        launch_request_id,
        activity_run_id,
        evidence_digest,
        disposition,
        safe_code,
        observed_at
      ) values (
        'genesis',
        v_source_key,
        p_launch_request_id,
        conflicting_run_id,
        p_genesis_evidence_digest,
        'conflict',
        'room_identity_conflict',
        sampled_at
      ) on conflict do nothing;
      return query select
        conflicting_run_id,
        null::text,
        'quarantined'::text,
        'conflict'::text,
        'room_identity_conflict'::text,
        null::text;
      return;
    end if;
  end if;

  identity_valid := coalesce(identity_valid
    and launch_row.roster_frozen_at is not null
    and launch_row.host_installation_id = evidence ->> 'host_installation_id'
    and launch_row.room_setup_operation_id = evidence ->> 'room_setup_operation_id'
    and launch_row.listing_revision_digest = evidence ->> 'listing_revision_digest'
    and ('sha256:' || encode(launch_row.frozen_roster_digest, 'hex'))
      = evidence ->> 'frozen_roster_digest'
    and launch_row.frozen_room_setup_specification_digest
      = evidence ->> 'room_setup_specification_digest'
    and listing_row.pack_revision_digest = evidence #>> '{pack,digest}'
    and listing_document -> 'pack' = evidence -> 'pack'
    and evidence #>> '{genesis_head,pack_digest}' = listing_row.pack_revision_digest,
    false
  );

  if frozen_roster is null
    or frozen_setup is null
    or jsonb_typeof(frozen_roster -> 'members') is distinct from 'array'
    or jsonb_typeof(frozen_setup -> 'spectators') is distinct from 'array'
  then
    membership_valid := false;
    expected_members := -1;
  else
    expected_members := jsonb_array_length(frozen_roster -> 'members')
      + jsonb_array_length(frozen_setup -> 'spectators');
    membership_valid := coalesce(
      expected_members = jsonb_array_length(evidence -> 'memberships'),
      false
    );
  end if;

  for member in select value from jsonb_array_elements(evidence -> 'memberships')
  loop
    v_purpose := member ->> 'purpose';
    v_source := null;
    v_expected_kind := null;
    v_controlling_account_id := null;
    v_house_agent_assignment_id := null;
    v_scope_digest := null;
    roster_member := null;
    listing_seat := null;

    if v_purpose = 'participant' then
      if member ->> 'access_mode' <> 'participant'
        or jsonb_typeof(member -> 'seat_id') <> 'string'
        or (member ->> 'seat_id') !~ '^[a-z][a-z0-9_-]{0,63}$'
        or jsonb_typeof(member -> 'role') <> 'string'
        or (member ->> 'role') !~ '^[a-z][a-z0-9_-]{0,63}$'
        or (member ? 'scopes' and member -> 'scopes' <> '[]'::jsonb)
      then
        membership_valid := false;
        continue;
      end if;

      select roster.value
      into roster_member
      from jsonb_array_elements(frozen_roster -> 'members') roster(value)
      where roster.value ->> 'seat_id' = member ->> 'seat_id';
      select seats.value
      into listing_seat
      from jsonb_array_elements(listing_row.seat_templates) seats(value)
      where seats.value ->> 'seat_id' = member ->> 'seat_id';
      if roster_member is null
        or listing_seat is null
        or listing_seat ->> 'role' <> member ->> 'role'
      then
        membership_valid := false;
        continue;
      end if;

      v_source := case roster_member ->> 'participation'
        when 'account_human' then 'account_human'
        when 'account_external_agent' then 'account_external_agent'
        when 'house_agent_fill' then 'platform_house_agent'
        else null
      end;
      v_expected_kind := case v_source
        when 'account_human' then 'human'
        when 'account_external_agent' then 'agent'
        when 'platform_house_agent' then 'agent'
        else null
      end;
      if v_source in ('account_human', 'account_external_agent') then
        select claims.controlling_account_id
        into v_controlling_account_id
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = member ->> 'seat_id'
          and claims.participation_kind = roster_member ->> 'participation'
          and claims.principal_reference = roster_member ->> 'principal_reference'
          and claims.released_at is null;
        if v_controlling_account_id is null then
          membership_valid := false;
        end if;
      elsif v_source = 'platform_house_agent' then
        select assignments.house_agent_assignment_id
        into v_house_agent_assignment_id
        from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.seat_id = member ->> 'seat_id'
          and assignments.setup_principal_reference = roster_member ->> 'principal_reference'
          and assignments.house_agent_revision_digest
            = roster_member ->> 'house_agent_revision_digest';
        if v_house_agent_assignment_id is null then
          membership_valid := false;
        end if;
      else
        membership_valid := false;
      end if;
      if member ->> 'principal_kind' is distinct from v_expected_kind then
        membership_valid := false;
      end if;
    else
      if member ->> 'access_mode' <> 'spectator'
        or jsonb_typeof(member -> 'seat_id') <> 'null'
        or jsonb_typeof(member -> 'role') <> 'null'
        or jsonb_typeof(member -> 'scopes') <> 'array'
      then
        membership_valid := false;
        continue;
      end if;
      v_setup_purpose := case v_purpose
        when 'creator_spectator' then 'creator'
        when 'result_indexer' then 'result_indexer'
        when 'public_projection_relay' then 'public_relay'
        else null
      end;
      if not exists (
        select 1
        from jsonb_array_elements(frozen_setup -> 'spectators') spectators(value)
        where spectators.value ->> 'purpose' = v_setup_purpose
      ) then
        membership_valid := false;
      end if;
      if v_purpose = 'creator_spectator' then
        v_source := 'account_human';
        v_expected_kind := 'human';
        v_controlling_account_id := launch_row.creator_account_id;
        if member -> 'scopes'
          <> '["room:attach","room:observe_public"]'::jsonb
        then
          membership_valid := false;
        end if;
      elsif v_purpose = 'result_indexer' then
        v_expected_kind := 'agent';
        if member -> 'scopes'
          <> '["room:attach","room:observe_public","room:replay"]'::jsonb
        then
          membership_valid := false;
        end if;
      elsif v_purpose = 'public_projection_relay' then
        v_expected_kind := 'agent';
        if member -> 'scopes'
          <> '["room:attach","room:observe_public"]'::jsonb
        then
          membership_valid := false;
        end if;
      end if;
      if member ->> 'principal_kind' is distinct from v_expected_kind then
        membership_valid := false;
      end if;
      v_scope_digest := extensions.digest(
        convert_to((member -> 'scopes')::text, 'utf8'),
        'sha256'
      );
    end if;

    if exists (
      select 1
      from platform_store.activity_run_memberships memberships
      where (
          memberships.membership_id = member ->> 'membership_id'
          or memberships.principal_id = member ->> 'principal_id'
        )
        and (v_run_id is null or memberships.activity_run_id <> v_run_id)
    ) then
      membership_valid := false;
    end if;

    derived_member := jsonb_build_object(
      'membership_id', member ->> 'membership_id',
      'access_mode', member ->> 'access_mode',
      'purpose', v_purpose,
      'seat_id', member ->> 'seat_id',
      'role', member ->> 'role',
      'public_seat_label', case
        when v_purpose = 'participant' then listing_seat ->> 'display_name'
        else null
      end,
      'principal_kind', member ->> 'principal_kind',
      'participation_source', v_source,
      'controlling_account_id', v_controlling_account_id,
      'principal_id', member ->> 'principal_id',
      'service_scope_digest', case
        when v_scope_digest is null then null
        else encode(v_scope_digest, 'hex')
      end,
      'house_agent_assignment_id', v_house_agent_assignment_id
    );
    derived_members := derived_members || jsonb_build_array(derived_member);
  end loop;

  if jsonb_array_length(derived_members) <> jsonb_array_length(evidence -> 'memberships') then
    membership_valid := false;
  end if;

  if v_run_id is not null and membership_valid then
    for derived_member in select value from jsonb_array_elements(derived_members)
    loop
      select memberships.*
      into existing_membership
      from platform_store.activity_run_memberships memberships
      where memberships.activity_run_id = v_run_id
        and (
          memberships.membership_id = derived_member ->> 'membership_id'
          or memberships.principal_id = derived_member ->> 'principal_id'
          or (
            derived_member ->> 'purpose' = 'participant'
            and memberships.seat_id = derived_member ->> 'seat_id'
          )
          or (
            derived_member ->> 'purpose' <> 'participant'
            and memberships.purpose = derived_member ->> 'purpose'
          )
        )
      limit 1;
      if found and (
        existing_membership.membership_id <> derived_member ->> 'membership_id'
        or existing_membership.access_mode <> derived_member ->> 'access_mode'
        or existing_membership.purpose <> derived_member ->> 'purpose'
        or existing_membership.seat_id is distinct from derived_member ->> 'seat_id'
        or existing_membership.role is distinct from derived_member ->> 'role'
        or existing_membership.public_seat_label
          is distinct from derived_member ->> 'public_seat_label'
        or existing_membership.principal_kind <> derived_member ->> 'principal_kind'
        or existing_membership.participation_source
          is distinct from derived_member ->> 'participation_source'
        or existing_membership.controlling_account_id
          is distinct from (derived_member ->> 'controlling_account_id')::uuid
        or existing_membership.principal_id <> derived_member ->> 'principal_id'
        or existing_membership.service_scope_digest is distinct from case
          when jsonb_typeof(derived_member -> 'service_scope_digest') = 'null' then null
          else decode(derived_member ->> 'service_scope_digest', 'hex')
        end
        or existing_membership.house_agent_assignment_id
          is distinct from (derived_member ->> 'house_agent_assignment_id')::uuid
        or (
          derived_member ->> 'controlling_account_id' is not null
          and existing_membership.entry_selector is null
        )
        or (
          derived_member ->> 'controlling_account_id' is null
          and existing_membership.entry_selector is not null
        )
      ) then
        correspondence_conflict := true;
      end if;
    end loop;
  end if;

  if correspondence_conflict then
    insert into platform_store.reconciliation_receipts (
      receipt_kind,
      source_key,
      launch_request_id,
      activity_run_id,
      evidence_digest,
      disposition,
      safe_code,
      observed_at
    ) values (
      'genesis',
      v_source_key,
      p_launch_request_id,
      v_run_id,
      p_genesis_evidence_digest,
      'conflict',
      'membership_correspondence_conflict',
      sampled_at
    ) on conflict do nothing;
    return query select
      v_run_id,
      existing_run.public_id,
      'quarantined'::text,
      'conflict'::text,
      'membership_correspondence_conflict'::text,
      existing_run.evidence_class;
    return;
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  join (
    select launch_row.creator_account_id as account_id
    union
    select claims.controlling_account_id
    from platform_store.seat_claims claims
    where claims.launch_request_id = p_launch_request_id
      and claims.released_at is null
  ) controlling on controlling.account_id = accounts.account_id
  order by accounts.account_id
  for update of accounts;

  select exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.launch_request_id = p_launch_request_id
      and reservations.kind = 'active_run'
      and reservations.reservation_class = 'authorized'
      and reservations.released_at is null
      and (reservations.activity_run_id is null or reservations.activity_run_id = v_run_id)
  ) into authorized_capacity_present;

  authorization_valid := coalesce(identity_valid
    and membership_valid
    and launch_row.host_mutation_started_at is not null
    and launch_row.state in ('provisioning', 'reconciling', 'run_created')
    and authorized_capacity_present
    and not exists (
      select 1
      from platform_store.platform_accounts accounts
      join (
        select launch_row.creator_account_id as account_id
        union
        select claims.controlling_account_id
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.released_at is null
      ) controlling on controlling.account_id = accounts.account_id
      where accounts.erased_at is not null
    ), false);

  v_evidence_class := case
    when exists (
      select 1
      from platform_store.house_agent_assignments assignments
      where assignments.launch_request_id = p_launch_request_id
    ) then 'exhibition_platform_house_agents'
    else 'unranked'
  end;

  if v_run_id is null then
    v_run_id := gen_random_uuid();
    v_initial_state := case
      when identity_valid and membership_valid and authorization_valid then 'ready'
      else 'quarantined'
    end;
    if listing_row.public_viewing_policy = 'anonymous_by_link'
      or listing_row.result_publication_policy = 'public_recent_results'
    then
      v_public_id := encode(extensions.gen_random_bytes(16), 'hex');
    end if;
    insert into platform_store.activity_runs (
      activity_run_id,
      launch_request_id,
      listing_revision_digest,
      creator_account_id,
      host_installation_id,
      room_setup_operation_id,
      room_id,
      launch_request_digest,
      pack_id,
      pack_version,
      pack_revision_digest,
      genesis_room_seq,
      genesis_or_transition_hash,
      canonical_genesis_evidence,
      genesis_evidence_digest,
      public_id,
      evidence_class,
      initial_reconciliation_state,
      genesis_observed_at,
      inserted_at
    ) values (
      v_run_id,
      p_launch_request_id,
      launch_row.listing_revision_digest,
      launch_row.creator_account_id,
      evidence ->> 'host_installation_id',
      evidence ->> 'room_setup_operation_id',
      evidence ->> 'room_id',
      evidence ->> 'launch_request_digest',
      evidence #>> '{pack,id}',
      evidence #>> '{pack,version}',
      evidence #>> '{pack,digest}',
      0,
      evidence #>> '{genesis_head,genesis_or_transition_hash}',
      p_canonical_genesis_evidence,
      p_genesis_evidence_digest,
      v_public_id,
      v_evidence_class,
      v_initial_state,
      sampled_at,
      sampled_at
    );
  else
    v_public_id := existing_run.public_id;
    v_initial_state := existing_run.initial_reconciliation_state;
    v_evidence_class := existing_run.evidence_class;
  end if;

  if membership_valid and v_initial_state in ('ready', 'quarantined') then
    for derived_member in select value from jsonb_array_elements(derived_members)
    loop
      if not exists (
        select 1
        from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = v_run_id
          and memberships.membership_id = derived_member ->> 'membership_id'
      ) then
        insert into platform_store.activity_run_memberships (
          activity_run_id,
          membership_id,
          access_mode,
          purpose,
          seat_id,
          role,
          public_seat_label,
          principal_kind,
          participation_source,
          controlling_account_id,
          principal_id,
          entry_selector,
          service_scope_digest,
          house_agent_assignment_id,
          promoted_at
        ) values (
          v_run_id,
          derived_member ->> 'membership_id',
          derived_member ->> 'access_mode',
          derived_member ->> 'purpose',
          derived_member ->> 'seat_id',
          derived_member ->> 'role',
          derived_member ->> 'public_seat_label',
          derived_member ->> 'principal_kind',
          derived_member ->> 'participation_source',
          (derived_member ->> 'controlling_account_id')::uuid,
          derived_member ->> 'principal_id',
          case
            when derived_member ->> 'controlling_account_id' is not null
              then encode(extensions.gen_random_bytes(16), 'hex')
            else null
          end,
          case
            when jsonb_typeof(derived_member -> 'service_scope_digest') = 'null' then null
            else decode(derived_member ->> 'service_scope_digest', 'hex')
          end,
          (derived_member ->> 'house_agent_assignment_id')::uuid,
          sampled_at
        );
        repaired_members := repaired_members + 1;
      end if;
    end loop;
  end if;

  select count(*)::integer
  into current_members
  from platform_store.activity_run_memberships memberships
  where memberships.activity_run_id = v_run_id;
  if membership_valid and current_members <> expected_members then
    insert into platform_store.reconciliation_receipts (
      receipt_kind,
      source_key,
      launch_request_id,
      activity_run_id,
      evidence_digest,
      disposition,
      safe_code,
      observed_at
    ) values (
      'genesis',
      v_source_key,
      p_launch_request_id,
      v_run_id,
      p_genesis_evidence_digest,
      'conflict',
      'membership_count_conflict',
      sampled_at
    ) on conflict do nothing;
    return query select
      v_run_id,
      v_public_id,
      'quarantined'::text,
      'conflict'::text,
      'membership_count_conflict'::text,
      v_evidence_class;
    return;
  end if;

  update platform_store.capacity_reservations reservations
  set activity_run_id = v_run_id
  where reservations.launch_request_id = p_launch_request_id
    and reservations.kind = 'active_run'
    and reservations.reservation_class = 'authorized'
    and reservations.released_at is null
    and reservations.activity_run_id is null;

  if not exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.launch_request_id = p_launch_request_id
      and reservations.kind = 'active_run'
      and reservations.released_at is null
      and reservations.activity_run_id = v_run_id
  ) then
    insert into platform_store.capacity_reservations (
      launch_request_id,
      kind,
      reservation_class,
      controlling_account_id,
      activity_run_id,
      reserved_at
    ) values (
      p_launch_request_id,
      'active_run',
      'quarantined_recovery',
      launch_row.creator_account_id,
      v_run_id,
      sampled_at
    );
  end if;

  update platform_store.capacity_reservations reservations
  set released_at = sampled_at,
      release_reason = 'genesis_observed'
  where reservations.launch_request_id = p_launch_request_id
    and reservations.kind = 'pre_genesis'
    and reservations.released_at is null;

  if launch_row.state in ('provisioning', 'reconciling') then
    update platform_store.launch_requests launches
    set state = 'run_created',
        last_transition_at = sampled_at
    where launches.launch_request_id = p_launch_request_id;
  end if;

  if v_initial_state = 'quarantined' or has_prior_conflict then
    v_disposition := case
      when existing_run.activity_run_id is null then 'blocked'
      else 'duplicate'
    end;
    v_safe_code := case
      when not identity_valid then 'genesis_identity_mismatch'
      when not membership_valid then 'membership_evidence_mismatch'
      when not authorization_valid then 'authorization_recovery_required'
      else 'genesis_quarantined'
    end;
    v_reconciliation_state := 'quarantined';
  elsif existing_run.activity_run_id is not null and repaired_members = 0 then
    v_disposition := 'duplicate';
    v_safe_code := 'genesis_already_recorded';
    v_reconciliation_state := 'ready';
  elsif existing_run.activity_run_id is not null then
    v_disposition := 'applied';
    v_safe_code := 'membership_correspondence_repaired';
    v_reconciliation_state := 'ready';
  else
    v_disposition := 'applied';
    v_safe_code := 'genesis_recorded';
    v_reconciliation_state := 'ready';
  end if;

  insert into platform_store.reconciliation_receipts (
    receipt_kind,
    source_key,
    launch_request_id,
    activity_run_id,
    evidence_digest,
    disposition,
    safe_code,
    observed_at
  ) values (
    'genesis',
    v_source_key,
    p_launch_request_id,
    v_run_id,
    p_genesis_evidence_digest,
    v_disposition,
    v_safe_code,
    sampled_at
  ) on conflict do nothing;

  return query select
    v_run_id,
    v_public_id,
    v_reconciliation_state,
    v_disposition,
    v_safe_code,
    v_evidence_class;
end;
$$;

revoke execute on function platform_api.record_genesis_v1(uuid, bytea, bytea)
  from public, anon, authenticated, service_role;
grant execute on function platform_api.record_genesis_v1(uuid, bytea, bytea)
  to service_role;
