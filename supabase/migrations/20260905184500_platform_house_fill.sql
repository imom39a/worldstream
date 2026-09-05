create table platform_store.house_agent_revisions (
  house_agent_revision_digest text primary key,
  house_agent_key text not null,
  display_name text not null,
  canonical_document bytea not null,
  model_slug text not null,
  provider_slug text not null,
  behavior_policy_id text not null,
  behavior_policy_revision text not null,
  agent_profile_id text not null,
  agent_profile_revision text not null,
  runner_template_id text not null,
  runner_template_revision text not null,
  accounting_tokenizer_id text not null,
  accounting_tokenizer_revision text not null,
  execution_allowance jsonb not null,
  tool_set jsonb not null,
  published_at timestamptz not null default clock_timestamp(),
  constraint house_agent_revision_digest_shape
    check (house_agent_revision_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint house_agent_key_shape check (
    length(house_agent_key) between 1 and 128
    and house_agent_key ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
  ),
  constraint house_agent_display_name_bounds check (
    length(display_name) between 1 and 128 and display_name !~ '[[:cntrl:]]'
  ),
  constraint house_agent_document_bounds
    check (octet_length(canonical_document) between 2 and 262144),
  constraint house_agent_model_shape check (
    length(model_slug) between 1 and 192
    and model_slug ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    and model_slug !~ '^~'
    and model_slug <> 'openrouter/auto'
  ),
  constraint house_agent_provider_shape check (
    length(provider_slug) between 1 and 192
    and provider_slug ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
  ),
  constraint house_agent_policy_shape check (
    length(behavior_policy_id) between 1 and 128
    and behavior_policy_id ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    and length(behavior_policy_revision) between 1 and 32
    and behavior_policy_revision ~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
  ),
  constraint house_agent_profile_shape check (
    length(agent_profile_id) between 1 and 128
    and agent_profile_id ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    and length(agent_profile_revision) between 1 and 32
    and agent_profile_revision ~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
  ),
  constraint house_agent_runner_shape check (
    length(runner_template_id) between 1 and 128
    and runner_template_id ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    and length(runner_template_revision) between 1 and 32
    and runner_template_revision ~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
  ),
  constraint house_agent_tokenizer_shape check (
    length(accounting_tokenizer_id) between 1 and 128
    and accounting_tokenizer_id ~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    and length(accounting_tokenizer_revision) between 1 and 32
    and accounting_tokenizer_revision ~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
  ),
  constraint house_agent_exact_allowance check (
    execution_allowance = '{
      "call_timeout_seconds": 60,
      "concurrent_calls": 1,
      "input_tokens_per_call": 12000,
      "model_call_attempts": 10,
      "output_tokens_per_call": 1000,
      "total_input_tokens": 120000,
      "total_output_tokens": 10000
    }'::jsonb
  ),
  constraint house_agent_empty_tools check (tool_set = '[]'::jsonb),
  constraint house_agent_unique_profile unique (agent_profile_id, agent_profile_revision)
);

create table platform_store.house_agent_host_approvals (
  host_installation_id text not null,
  house_agent_revision_digest text not null
    references platform_store.house_agent_revisions(house_agent_revision_digest)
    on delete restrict,
  agent_profile_revision_digest text not null,
  runner_template_revision_digest text not null,
  runner_executable_digest text not null,
  named_credential_reference text not null,
  approval_receipt_digest bytea not null,
  available_for_new_assignments boolean not null default false,
  approved_at timestamptz not null default clock_timestamp(),
  availability_checked_at timestamptz not null default clock_timestamp(),
  revoked_at timestamptz,
  primary key (host_installation_id, house_agent_revision_digest),
  constraint house_agent_approval_host_shape check (
    host_installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
  ),
  constraint house_agent_approval_profile_digest
    check (agent_profile_revision_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint house_agent_approval_runner_digest
    check (runner_template_revision_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint house_agent_approval_executable_digest
    check (runner_executable_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint house_agent_approval_credential_shape check (
    length(named_credential_reference) between 1 and 128
    and named_credential_reference ~ '^[A-Za-z0-9][A-Za-z0-9._:-]*$'
  ),
  constraint house_agent_approval_receipt_digest
    check (octet_length(approval_receipt_digest) = 32),
  constraint house_agent_approval_time_order check (
    availability_checked_at >= approved_at
    and (revoked_at is null or revoked_at >= approved_at)
    and not (revoked_at is not null and available_for_new_assignments)
  )
);

create table platform_store.house_runner_capacity_gates (
  gate_kind text primary key,
  hard_limit integer not null,
  created_at timestamptz not null default clock_timestamp(),
  constraint house_runner_gate_single_kind check (gate_kind = 'runner_unit'),
  constraint house_runner_gate_exact_limit check (hard_limit = 4)
);

insert into platform_store.house_runner_capacity_gates(gate_kind, hard_limit)
values ('runner_unit', 4);

create table platform_store.house_fill_operations (
  house_fill_operation_id uuid primary key default gen_random_uuid(),
  launch_request_id uuid not null unique
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  listing_revision_digest text not null
    references platform_store.activity_listing_revisions(listing_revision_digest)
    on delete restrict,
  state text not null default 'claim_window_open',
  claim_window_opened_at timestamptz not null default clock_timestamp(),
  claim_window_closes_at timestamptz not null,
  host_installation_id text,
  selection_retained_at timestamptz,
  candidate_set_digest bytea,
  exclusion_set_digest bytea,
  selected_assignment_count integer,
  completed_at timestamptz,
  failure_code text,
  constraint house_fill_operation_state check (state in (
    'claim_window_open',
    'reserving',
    'assignments_complete',
    'failed_pre_genesis'
  )),
  constraint house_fill_exact_claim_window check (
    claim_window_closes_at = claim_window_opened_at + interval '30 seconds'
  ),
  constraint house_fill_host_shape check (
    host_installation_id is null
    or host_installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
  ),
  constraint house_fill_selection_shape check (
    (selection_retained_at is null
      and host_installation_id is null
      and candidate_set_digest is null
      and exclusion_set_digest is null
      and selected_assignment_count is null)
    or
    (selection_retained_at is not null
      and selection_retained_at >= claim_window_closes_at
      and host_installation_id is not null
      and candidate_set_digest is not null
      and octet_length(candidate_set_digest) = 32
      and exclusion_set_digest is not null
      and octet_length(exclusion_set_digest) = 32
      and selected_assignment_count between 0 and 2)
  ),
  constraint house_fill_completion_shape check (
    (state = 'claim_window_open'
      and selection_retained_at is null
      and completed_at is null
      and failure_code is null)
    or
    (state = 'reserving'
      and selection_retained_at is not null
      and selected_assignment_count > 0
      and completed_at is null
      and failure_code is null)
    or
    (state = 'assignments_complete'
      and selection_retained_at is not null
      and completed_at >= selection_retained_at
      and failure_code is null)
    or
    (state = 'failed_pre_genesis'
      and completed_at is not null
      and failure_code is not null
      and length(failure_code) between 1 and 64
      and failure_code ~ '^[a-z][a-z0-9_]*$')
  )
);

create table platform_store.house_fill_candidate_evidence (
  house_fill_operation_id uuid not null
    references platform_store.house_fill_operations(house_fill_operation_id)
    on delete restrict,
  house_agent_revision_digest text not null
    references platform_store.house_agent_revisions(house_agent_revision_digest)
    on delete restrict,
  disposition text not null,
  exclusion_reason text,
  random_draw bytea,
  selected_seat_id text,
  selection_rank integer,
  retained_at timestamptz not null,
  primary key (house_fill_operation_id, house_agent_revision_digest),
  constraint house_fill_candidate_disposition
    check (disposition in ('candidate', 'excluded')),
  constraint house_fill_candidate_shape check (
    (disposition = 'candidate'
      and exclusion_reason is null
      and random_draw is not null
      and octet_length(random_draw) = 32)
    or
    (disposition = 'excluded'
      and exclusion_reason in (
        'not_listing_allowlisted',
        'host_not_approved',
        'host_unavailable',
        'already_selected_in_lineage'
      )
      and random_draw is null
      and selected_seat_id is null
      and selection_rank is null)
  ),
  constraint house_fill_candidate_selection_shape check (
    (selected_seat_id is null and selection_rank is null)
    or
    (disposition = 'candidate'
      and selected_seat_id is not null
      and selected_seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'
      and selection_rank is not null
      and selection_rank between 1 and 2)
  ),
  unique (house_fill_operation_id, selected_seat_id),
  unique (house_fill_operation_id, selection_rank)
);

create table platform_store.house_runner_reservations (
  reservation_operation_id uuid primary key default gen_random_uuid(),
  house_fill_operation_id uuid not null
    references platform_store.house_fill_operations(house_fill_operation_id)
    on delete restrict,
  launch_request_id uuid not null
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  seat_id text not null,
  house_agent_revision_digest text not null
    references platform_store.house_agent_revisions(house_agent_revision_digest)
    on delete restrict,
  state text not null default 'pending',
  runner_unit_id text,
  reservation_receipt bytea,
  reservation_receipt_digest bytea,
  failure_code text,
  reserved_at timestamptz not null default clock_timestamp(),
  last_transition_at timestamptz not null default clock_timestamp(),
  released_at timestamptz,
  release_receipt_digest bytea,
  constraint house_runner_reservation_seat_shape
    check (seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'),
  constraint house_runner_reservation_state check (state in (
    'pending', 'ambiguous', 'succeeded', 'terminal_failed', 'released'
  )),
  constraint house_runner_reservation_time_order check (
    last_transition_at >= reserved_at
    and (released_at is null or released_at >= reserved_at)
  ),
  constraint house_runner_reservation_result_shape check (
    (state in ('pending', 'ambiguous')
      and runner_unit_id is null
      and reservation_receipt is null
      and reservation_receipt_digest is null
      and failure_code is null
      and released_at is null
      and release_receipt_digest is null)
    or
    (state = 'succeeded'
      and runner_unit_id is not null
      and runner_unit_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
      and reservation_receipt is not null
      and octet_length(reservation_receipt) between 2 and 8192
      and reservation_receipt_digest is not null
      and octet_length(reservation_receipt_digest) = 32
      and failure_code is null
      and released_at is null
      and release_receipt_digest is null)
    or
    (state = 'terminal_failed'
      and runner_unit_id is null
      and reservation_receipt is not null
      and octet_length(reservation_receipt) between 2 and 8192
      and reservation_receipt_digest is not null
      and octet_length(reservation_receipt_digest) = 32
      and failure_code is not null
      and length(failure_code) between 1 and 64
      and failure_code ~ '^[a-z][a-z0-9_]*$'
      and released_at is null
      and release_receipt_digest is null)
    or
    (state = 'released'
      and runner_unit_id is not null
      and octet_length(reservation_receipt) between 2 and 8192
      and octet_length(reservation_receipt_digest) = 32
      and failure_code is null
      and released_at is not null
      and octet_length(release_receipt_digest) = 32)
  ),
  unique (house_fill_operation_id, seat_id),
  unique (house_fill_operation_id, house_agent_revision_digest)
);

create index house_runner_reservations_live_idx
  on platform_store.house_runner_reservations (state, released_at, reservation_operation_id)
  where state in ('pending', 'ambiguous', 'succeeded') and released_at is null;

create table platform_store.house_agent_assignments (
  house_agent_assignment_id uuid primary key default gen_random_uuid(),
  launch_request_id uuid not null
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  house_fill_operation_id uuid not null
    references platform_store.house_fill_operations(house_fill_operation_id)
    on delete restrict,
  listing_revision_digest text not null
    references platform_store.activity_listing_revisions(listing_revision_digest)
    on delete restrict,
  seat_id text not null,
  setup_principal_reference text not null,
  house_agent_revision_digest text not null
    references platform_store.house_agent_revisions(house_agent_revision_digest)
    on delete restrict,
  model_slug text not null,
  provider_slug text not null,
  behavior_policy_id text not null,
  behavior_policy_revision text not null,
  agent_profile_id text not null,
  agent_profile_revision text not null,
  runner_template_id text not null,
  runner_template_revision text not null,
  accounting_tokenizer_id text not null,
  accounting_tokenizer_revision text not null,
  execution_allowance jsonb not null,
  selection_evidence_digest bytea not null,
  reservation_operation_id uuid not null unique
    references platform_store.house_runner_reservations(reservation_operation_id)
    on delete restrict,
  reservation_receipt_digest bytea not null,
  assigned_at timestamptz not null default clock_timestamp(),
  constraint house_agent_assignment_seat_shape
    check (seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'),
  constraint house_agent_assignment_principal_shape check (
    length(setup_principal_reference) between 1 and 128
    and setup_principal_reference ~ '^[A-Za-z0-9][A-Za-z0-9._:-]*$'
  ),
  constraint house_agent_assignment_selection_digest
    check (octet_length(selection_evidence_digest) = 32),
  constraint house_agent_assignment_receipt_digest
    check (octet_length(reservation_receipt_digest) = 32),
  unique (launch_request_id, seat_id),
  unique (launch_request_id, house_agent_revision_digest),
  unique (house_fill_operation_id, seat_id),
  unique (house_fill_operation_id, house_agent_revision_digest)
);

alter table platform_store.house_agent_revisions enable row level security;
alter table platform_store.house_agent_host_approvals enable row level security;
alter table platform_store.house_runner_capacity_gates enable row level security;
alter table platform_store.house_fill_operations enable row level security;
alter table platform_store.house_fill_candidate_evidence enable row level security;
alter table platform_store.house_runner_reservations enable row level security;
alter table platform_store.house_agent_assignments enable row level security;

create trigger protect_house_agent_revision_v1
before update or delete on platform_store.house_agent_revisions
for each row execute function platform_store.reject_immutable_formation_row_v1();

create trigger protect_house_runner_capacity_gate_v1
before update or delete on platform_store.house_runner_capacity_gates
for each row execute function platform_store.reject_immutable_formation_row_v1();

create trigger protect_house_agent_assignment_v1
before update or delete on platform_store.house_agent_assignments
for each row execute function platform_store.reject_immutable_formation_row_v1();

create function platform_store.protect_house_agent_host_approval_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE'
    or new.host_installation_id <> old.host_installation_id
    or new.house_agent_revision_digest <> old.house_agent_revision_digest
    or new.agent_profile_revision_digest <> old.agent_profile_revision_digest
    or new.runner_template_revision_digest <> old.runner_template_revision_digest
    or new.runner_executable_digest <> old.runner_executable_digest
    or new.named_credential_reference <> old.named_credential_reference
    or new.approval_receipt_digest <> old.approval_receipt_digest
    or new.approved_at <> old.approved_at
    or new.availability_checked_at < old.availability_checked_at
    or old.revoked_at is not null
       and new.revoked_at is distinct from old.revoked_at
  then
    raise exception using errcode = '23000', message = 'immutable_house_agent_approval';
  end if;
  return new;
end;
$$;

create trigger protect_house_agent_host_approval_v1
before update or delete on platform_store.house_agent_host_approvals
for each row execute function platform_store.protect_house_agent_host_approval_v1();

create function platform_store.protect_house_fill_operation_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE'
    or new.house_fill_operation_id <> old.house_fill_operation_id
    or new.launch_request_id <> old.launch_request_id
    or new.listing_revision_digest <> old.listing_revision_digest
    or new.claim_window_opened_at <> old.claim_window_opened_at
    or new.claim_window_closes_at <> old.claim_window_closes_at
    or old.host_installation_id is not null
       and new.host_installation_id is distinct from old.host_installation_id
    or old.selection_retained_at is not null and (
      new.selection_retained_at is distinct from old.selection_retained_at
      or new.candidate_set_digest is distinct from old.candidate_set_digest
      or new.exclusion_set_digest is distinct from old.exclusion_set_digest
      or new.selected_assignment_count is distinct from old.selected_assignment_count
    )
    or old.completed_at is not null and (
      new.completed_at is distinct from old.completed_at
      or new.failure_code is distinct from old.failure_code
    )
  then
    raise exception using errcode = '23000', message = 'immutable_house_fill_operation';
  end if;

  if new.state <> old.state and not (
    (old.state = 'claim_window_open'
      and new.state in ('reserving', 'assignments_complete', 'failed_pre_genesis'))
    or (old.state = 'reserving'
      and new.state in ('assignments_complete', 'failed_pre_genesis'))
  ) then
    raise exception using errcode = '23000', message = 'invalid_house_fill_transition';
  end if;
  return new;
end;
$$;

create trigger protect_house_fill_operation_v1
before update or delete on platform_store.house_fill_operations
for each row execute function platform_store.protect_house_fill_operation_v1();

create function platform_store.protect_house_fill_candidate_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
declare
  operation_state text;
begin
  if tg_op = 'DELETE' then
    raise exception using errcode = '23000', message = 'immutable_house_fill_candidate';
  end if;
  if tg_op = 'UPDATE' then
    select operations.state into operation_state
    from platform_store.house_fill_operations operations
    where operations.house_fill_operation_id = old.house_fill_operation_id;
    if operation_state <> 'claim_window_open'
      or new.house_fill_operation_id <> old.house_fill_operation_id
      or new.house_agent_revision_digest <> old.house_agent_revision_digest
      or new.disposition <> old.disposition
      or new.exclusion_reason is distinct from old.exclusion_reason
      or new.random_draw is distinct from old.random_draw
      or new.retained_at <> old.retained_at
      or old.selected_seat_id is not null
      or old.selection_rank is not null
    then
      raise exception using errcode = '23000', message = 'immutable_house_fill_candidate';
    end if;
  end if;
  return new;
end;
$$;

create trigger protect_house_fill_candidate_v1
before update or delete on platform_store.house_fill_candidate_evidence
for each row execute function platform_store.protect_house_fill_candidate_v1();

create function platform_store.protect_house_runner_reservation_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE'
    or new.reservation_operation_id <> old.reservation_operation_id
    or new.house_fill_operation_id <> old.house_fill_operation_id
    or new.launch_request_id <> old.launch_request_id
    or new.seat_id <> old.seat_id
    or new.house_agent_revision_digest <> old.house_agent_revision_digest
    or new.reserved_at <> old.reserved_at
    or new.last_transition_at < old.last_transition_at
    or old.state in ('terminal_failed', 'released') and new.state <> old.state
    or old.runner_unit_id is not null and new.runner_unit_id is distinct from old.runner_unit_id
    or old.reservation_receipt is not null
       and new.reservation_receipt is distinct from old.reservation_receipt
    or old.reservation_receipt_digest is not null
       and new.reservation_receipt_digest is distinct from old.reservation_receipt_digest
    or old.failure_code is not null and new.failure_code is distinct from old.failure_code
    or old.released_at is not null and (
      new.released_at is distinct from old.released_at
      or new.release_receipt_digest is distinct from old.release_receipt_digest
    )
  then
    raise exception using errcode = '23000', message = 'immutable_house_runner_reservation';
  end if;
  if new.state <> old.state and not (
    old.state = 'pending' and new.state in ('ambiguous', 'succeeded', 'terminal_failed')
    or old.state = 'ambiguous' and new.state in ('succeeded', 'terminal_failed')
    or old.state = 'succeeded' and new.state = 'released'
  ) then
    raise exception using errcode = '23000', message = 'invalid_house_runner_transition';
  end if;
  return new;
end;
$$;

create trigger protect_house_runner_reservation_v1
before update or delete on platform_store.house_runner_reservations
for each row execute function platform_store.protect_house_runner_reservation_v1();

create function platform_store.house_fill_claims_closed_v1(p_launch_request_id uuid)
returns boolean
language sql
stable
security invoker
set search_path = ''
as $$
  select exists (
    select 1
    from platform_store.house_fill_operations operations
    where operations.launch_request_id = p_launch_request_id
      and operations.claim_window_closes_at <= clock_timestamp()
  );
$$;

create function platform_store.reject_closed_house_fill_claim_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if platform_store.house_fill_claims_closed_v1(new.launch_request_id) then
    raise exception using errcode = '55000', message = 'house_fill_claim_window_closed';
  end if;
  return new;
end;
$$;

create trigger reject_closed_house_fill_claim_v1
before insert or update on platform_store.seat_claims
for each row execute function platform_store.reject_closed_house_fill_claim_v1();

create function platform_store.house_fill_operation_json_v1(p_launch_request_id uuid)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select jsonb_build_object(
    'version', 'platform_house_fill_operation.v1',
    'house_fill_operation_id', operations.house_fill_operation_id,
    'launch_request_id', operations.launch_request_id,
    'listing_revision_digest', operations.listing_revision_digest,
    'state', operations.state,
    'claim_window_closes_at', operations.claim_window_closes_at,
    'selected_assignment_count', operations.selected_assignment_count,
    'failure_code', operations.failure_code,
    'reservations', coalesce((
      select jsonb_agg(jsonb_build_object(
        'reservation_operation_id', reservations.reservation_operation_id,
        'seat_id', reservations.seat_id,
        'house_agent_revision_digest', reservations.house_agent_revision_digest,
        'state', reservations.state,
        'runner_unit_id', reservations.runner_unit_id
      ) order by reservations.seat_id)
      from platform_store.house_runner_reservations reservations
      where reservations.house_fill_operation_id = operations.house_fill_operation_id
    ), '[]'::jsonb),
    'assignments', coalesce((
      select jsonb_agg(jsonb_build_object(
        'seat_id', assignments.seat_id,
        'principal_reference', assignments.setup_principal_reference,
        'display_name', revisions.display_name,
        'house_agent_revision_digest', assignments.house_agent_revision_digest,
        'model_slug', assignments.model_slug,
        'provider_slug', assignments.provider_slug,
        'agent_profile', jsonb_build_object(
          'profile_id', assignments.agent_profile_id,
          'revision', assignments.agent_profile_revision
        ),
        'runner_template', jsonb_build_object(
          'template_id', assignments.runner_template_id,
          'revision', assignments.runner_template_revision
        ),
        'execution_allowance', assignments.execution_allowance
      ) order by assignments.seat_id)
      from platform_store.house_agent_assignments assignments
      join platform_store.house_agent_revisions revisions
        on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
      where assignments.house_fill_operation_id = operations.house_fill_operation_id
    ), '[]'::jsonb)
  )
  from platform_store.house_fill_operations operations
  where operations.launch_request_id = p_launch_request_id;
$$;

create function platform_store.fail_house_fill_v1(
  p_house_fill_operation_id uuid,
  p_failure_code text,
  p_host_installation_id text default null,
  p_candidate_set_digest bytea default null,
  p_exclusion_set_digest bytea default null,
  p_selected_assignment_count integer default null
)
returns void
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_id uuid;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_failure_code !~ '^[a-z][a-z0-9_]{0,63}$' then
    raise exception using errcode = '22023', message = 'invalid_house_fill_failure';
  end if;
  if not (
    (p_host_installation_id is null
      and p_candidate_set_digest is null
      and p_exclusion_set_digest is null
      and p_selected_assignment_count is null)
    or
    (p_host_installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
      and octet_length(p_candidate_set_digest) = 32
      and octet_length(p_exclusion_set_digest) = 32
      and p_selected_assignment_count between 0 and 2)
  ) then
    raise exception using errcode = '22023', message = 'invalid_house_fill_failure_evidence';
  end if;
  update platform_store.house_fill_operations operations
  set state = 'failed_pre_genesis',
      host_installation_id = coalesce(
        operations.host_installation_id,
        p_host_installation_id
      ),
      selection_retained_at = case
        when operations.selection_retained_at is not null
          then operations.selection_retained_at
        when p_host_installation_id is not null then sampled_at
        else null
      end,
      candidate_set_digest = coalesce(
        operations.candidate_set_digest,
        p_candidate_set_digest
      ),
      exclusion_set_digest = coalesce(
        operations.exclusion_set_digest,
        p_exclusion_set_digest
      ),
      selected_assignment_count = coalesce(
        operations.selected_assignment_count,
        p_selected_assignment_count
      ),
      completed_at = sampled_at,
      failure_code = p_failure_code
  where operations.house_fill_operation_id = p_house_fill_operation_id
    and operations.state in ('claim_window_open', 'reserving')
  returning operations.launch_request_id into launch_id;
  if launch_id is null then
    return;
  end if;
  perform platform_store.release_launch_capacity_v1(launch_id, 'failed_pre_genesis');
  update platform_store.launch_requests launches
  set state = 'failed_pre_genesis', last_transition_at = sampled_at
  where launches.launch_request_id = launch_id
    and launches.state = 'collecting_roster'
    and launches.roster_frozen_at is null
    and launches.host_mutation_started_at is null;
end;
$$;

create function platform_api.start_house_fill_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  existing jsonb;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found or launch_row.creator_account_id <> p_creator_account_id then
    return null;
  end if;

  select platform_store.house_fill_operation_json_v1(p_launch_request_id)
  into existing;
  if existing is not null then
    return existing;
  end if;
  if launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.expires_at <= sampled_at + interval '30 seconds'
    or launch_row.house_fill_choice <> 'fill_unclaimed'
  then
    raise exception using errcode = '55000', message = 'house_fill_unavailable';
  end if;
  if not exists (
    select 1
    from platform_store.activity_listing_revisions listings,
         jsonb_array_elements(listings.seat_templates) seats(value)
    where listings.listing_revision_digest = launch_row.listing_revision_digest
      and (seats.value -> 'allowed_participation') ? 'house_agent_fill'
  ) then
    raise exception using errcode = '55000', message = 'house_fill_unavailable';
  end if;

  insert into platform_store.house_fill_operations (
    launch_request_id,
    listing_revision_digest,
    claim_window_opened_at,
    claim_window_closes_at
  ) values (
    p_launch_request_id,
    launch_row.listing_revision_digest,
    sampled_at,
    sampled_at + interval '30 seconds'
  );
  return platform_store.house_fill_operation_json_v1(p_launch_request_id);
end;
$$;

create function platform_api.retain_house_fill_selection_v1(
  p_launch_request_id uuid,
  p_host_installation_id text
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  operation_row platform_store.house_fill_operations%rowtype;
  seat_record record;
  selected_revision text;
  selected_rank integer := 0;
  selected_count integer;
  candidate_count integer;
  active_runner_count integer;
  runner_limit integer;
  sampled_at timestamptz := clock_timestamp();
  candidate_digest bytea;
  exclusion_digest bytea;
begin
  if p_host_installation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$' then
    raise exception using errcode = '22023', message = 'invalid_host_installation';
  end if;
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then
    return null;
  end if;
  select operations.* into operation_row
  from platform_store.house_fill_operations operations
  where operations.launch_request_id = p_launch_request_id
  for update;
  if not found then
    raise exception using errcode = '55000', message = 'house_fill_not_started';
  end if;
  if operation_row.state <> 'claim_window_open' then
    if operation_row.host_installation_id is not null
      and operation_row.host_installation_id <> p_host_installation_id
    then
      raise exception using errcode = '23505', message = 'house_fill_host_conflict';
    end if;
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if operation_row.claim_window_closes_at > sampled_at then
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.host_mutation_started_at is not null
    or launch_row.expires_at <= sampled_at
  then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'launch_unavailable'
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  select listings.* into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  if exists (
    select 1
    from jsonb_array_elements(listing_row.seat_templates) seats(value)
    where (seats.value ->> 'required')::boolean
      and not exists (
        select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = seats.value ->> 'seat_id'
          and claims.released_at is null
      )
      and not ((seats.value -> 'allowed_participation') ? 'house_agent_fill')
  ) then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'required_seat_not_fillable'
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  select count(*)::integer into selected_count
  from jsonb_array_elements(listing_row.seat_templates) seats(value)
  where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
    and not exists (
      select 1 from platform_store.seat_claims claims
      where claims.launch_request_id = p_launch_request_id
        and claims.seat_id = seats.value ->> 'seat_id'
        and claims.released_at is null
    );
  if selected_count > 2 then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'assignment_limit_exceeded'
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if selected_count = 0 then
    candidate_digest := extensions.digest(convert_to('[]', 'utf8'), 'sha256');
    exclusion_digest := extensions.digest(convert_to('[]', 'utf8'), 'sha256');
    update platform_store.house_fill_operations operations
    set host_installation_id = p_host_installation_id,
        selection_retained_at = sampled_at,
        candidate_set_digest = candidate_digest,
        exclusion_set_digest = exclusion_digest,
        selected_assignment_count = 0,
        state = 'assignments_complete',
        completed_at = sampled_at
    where operations.house_fill_operation_id = operation_row.house_fill_operation_id;
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  insert into platform_store.house_fill_candidate_evidence (
    house_fill_operation_id,
    house_agent_revision_digest,
    disposition,
    exclusion_reason,
    random_draw,
    retained_at
  )
  select
    operation_row.house_fill_operation_id,
    revisions.house_agent_revision_digest,
    case
      when exists (
        select 1
        from jsonb_array_elements(listing_row.seat_templates) seats(value)
        where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
          and (seats.value -> 'allowed_house_agent_revisions')
            ? revisions.house_agent_revision_digest
          and not exists (
            select 1 from platform_store.seat_claims claims
            where claims.launch_request_id = p_launch_request_id
              and claims.seat_id = seats.value ->> 'seat_id'
              and claims.released_at is null
          )
      )
      and approvals.house_agent_revision_digest is not null
      and approvals.revoked_at is null
      and approvals.available_for_new_assignments
      and not exists (
        select 1 from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.house_agent_revision_digest = revisions.house_agent_revision_digest
      ) then 'candidate'
      else 'excluded'
    end,
    case
      when not exists (
        select 1
        from jsonb_array_elements(listing_row.seat_templates) seats(value)
        where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
          and (seats.value -> 'allowed_house_agent_revisions')
            ? revisions.house_agent_revision_digest
          and not exists (
            select 1 from platform_store.seat_claims claims
            where claims.launch_request_id = p_launch_request_id
              and claims.seat_id = seats.value ->> 'seat_id'
              and claims.released_at is null
          )
      ) then 'not_listing_allowlisted'
      when approvals.house_agent_revision_digest is null or approvals.revoked_at is not null
        then 'host_not_approved'
      when not approvals.available_for_new_assignments then 'host_unavailable'
      when exists (
        select 1 from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.house_agent_revision_digest = revisions.house_agent_revision_digest
      ) then 'already_selected_in_lineage'
      else null
    end,
    case
      when approvals.house_agent_revision_digest is not null
        and approvals.revoked_at is null
        and approvals.available_for_new_assignments
        and exists (
          select 1
          from jsonb_array_elements(listing_row.seat_templates) seats(value)
          where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
            and (seats.value -> 'allowed_house_agent_revisions')
              ? revisions.house_agent_revision_digest
            and not exists (
              select 1 from platform_store.seat_claims claims
              where claims.launch_request_id = p_launch_request_id
                and claims.seat_id = seats.value ->> 'seat_id'
                and claims.released_at is null
            )
        )
        and not exists (
          select 1 from platform_store.house_agent_assignments assignments
          where assignments.launch_request_id = p_launch_request_id
            and assignments.house_agent_revision_digest = revisions.house_agent_revision_digest
        )
      then extensions.gen_random_bytes(32)
      else null
    end,
    sampled_at
  from platform_store.house_agent_revisions revisions
  left join platform_store.house_agent_host_approvals approvals
    on approvals.house_agent_revision_digest = revisions.house_agent_revision_digest
   and approvals.host_installation_id = p_host_installation_id;

  select count(*)::integer into candidate_count
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.disposition = 'candidate';
  if candidate_count < selected_count then
    select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
      'revision', candidates.house_agent_revision_digest,
      'draw', encode(candidates.random_draw, 'hex')
    ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
    into candidate_digest
    from platform_store.house_fill_candidate_evidence candidates
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.disposition = 'candidate';
    select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
      'revision', candidates.house_agent_revision_digest,
      'reason', candidates.exclusion_reason
    ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
    into exclusion_digest
    from platform_store.house_fill_candidate_evidence candidates
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.disposition = 'excluded';
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'insufficient_candidates',
      p_host_installation_id,
      candidate_digest,
      exclusion_digest,
      selected_count
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  for seat_record in
    select seats.value, seats.ordinality,
      (
        select count(*)
        from platform_store.house_fill_candidate_evidence candidates
        where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
          and candidates.disposition = 'candidate'
          and (seats.value -> 'allowed_house_agent_revisions')
            ? candidates.house_agent_revision_digest
      ) as compatible_count
    from jsonb_array_elements(listing_row.seat_templates)
         with ordinality seats(value, ordinality)
    where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
      and not exists (
        select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = seats.value ->> 'seat_id'
          and claims.released_at is null
      )
    order by compatible_count, seats.ordinality
  loop
    select candidates.house_agent_revision_digest into selected_revision
    from platform_store.house_fill_candidate_evidence candidates
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.disposition = 'candidate'
      and candidates.selected_seat_id is null
      and (seat_record.value -> 'allowed_house_agent_revisions')
        ? candidates.house_agent_revision_digest
    order by candidates.random_draw, candidates.house_agent_revision_digest
    limit 1;
    if selected_revision is null then
      select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
        'revision', candidates.house_agent_revision_digest,
        'draw', encode(candidates.random_draw, 'hex')
      ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
      into candidate_digest
      from platform_store.house_fill_candidate_evidence candidates
      where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
        and candidates.disposition = 'candidate';
      select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
        'revision', candidates.house_agent_revision_digest,
        'reason', candidates.exclusion_reason
      ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
      into exclusion_digest
      from platform_store.house_fill_candidate_evidence candidates
      where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
        and candidates.disposition = 'excluded';
      perform platform_store.fail_house_fill_v1(
        operation_row.house_fill_operation_id,
        'incompatible_candidate_set',
        p_host_installation_id,
        candidate_digest,
        exclusion_digest,
        selected_count
      );
      return platform_store.house_fill_operation_json_v1(p_launch_request_id);
    end if;
    selected_rank := selected_rank + 1;
    update platform_store.house_fill_candidate_evidence candidates
    set selected_seat_id = seat_record.value ->> 'seat_id', selection_rank = selected_rank
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.house_agent_revision_digest = selected_revision;
    selected_revision := null;
  end loop;

  select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
    'revision', candidates.house_agent_revision_digest,
    'draw', encode(candidates.random_draw, 'hex'),
    'seat', candidates.selected_seat_id
  ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
  into candidate_digest
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.disposition = 'candidate';
  select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
    'revision', candidates.house_agent_revision_digest,
    'reason', candidates.exclusion_reason
  ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
  into exclusion_digest
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.disposition = 'excluded';

  select gates.hard_limit into strict runner_limit
  from platform_store.house_runner_capacity_gates gates
  where gates.gate_kind = 'runner_unit'
  for update;
  perform reservations.reservation_operation_id
  from platform_store.house_runner_reservations reservations
  where reservations.state in ('pending', 'ambiguous', 'succeeded')
    and reservations.released_at is null
  order by reservations.reservation_operation_id
  for update;
  select count(*)::integer into active_runner_count
  from platform_store.house_runner_reservations reservations
  where reservations.state in ('pending', 'ambiguous', 'succeeded')
    and reservations.released_at is null;
  if active_runner_count + selected_count > runner_limit then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'runner_capacity_unavailable',
      p_host_installation_id,
      candidate_digest,
      exclusion_digest,
      selected_count
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  insert into platform_store.house_runner_reservations (
    house_fill_operation_id,
    launch_request_id,
    seat_id,
    house_agent_revision_digest,
    reserved_at,
    last_transition_at
  )
  select
    operation_row.house_fill_operation_id,
    p_launch_request_id,
    candidates.selected_seat_id,
    candidates.house_agent_revision_digest,
    sampled_at,
    sampled_at
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.selected_seat_id is not null
  order by candidates.selection_rank;

  update platform_store.house_fill_operations operations
  set host_installation_id = p_host_installation_id,
      selection_retained_at = sampled_at,
      candidate_set_digest = candidate_digest,
      exclusion_set_digest = exclusion_digest,
      selected_assignment_count = selected_count,
      state = 'reserving'
  where operations.house_fill_operation_id = operation_row.house_fill_operation_id;
  return platform_store.house_fill_operation_json_v1(p_launch_request_id);
end;
$$;

create function platform_api.record_house_runner_reservation_v1(
  p_reservation_operation_id uuid,
  p_outcome text,
  p_runner_unit_id text,
  p_reservation_receipt bytea,
  p_reservation_receipt_digest bytea,
  p_failure_code text
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  reservation_row platform_store.house_runner_reservations%rowtype;
  sampled_at timestamptz := clock_timestamp();
  parsed_receipt jsonb;
begin
  if p_outcome not in ('ambiguous', 'succeeded', 'terminal_failed') then
    raise exception using errcode = '22023', message = 'invalid_reservation_outcome';
  end if;
  if p_outcome = 'ambiguous' then
    if p_runner_unit_id is not null
      or p_reservation_receipt is not null
      or p_reservation_receipt_digest is not null
      or p_failure_code is not null
    then
      raise exception using errcode = '22023', message = 'invalid_reservation_result';
    end if;
  else
    if octet_length(p_reservation_receipt) not between 2 and 8192
      or octet_length(p_reservation_receipt_digest) <> 32
      or p_reservation_receipt_digest <> extensions.digest(p_reservation_receipt, 'sha256')
    then
      raise exception using errcode = '22023', message = 'invalid_reservation_receipt';
    end if;
    begin
      parsed_receipt := convert_from(p_reservation_receipt, 'utf8')::jsonb;
    exception when others then
      raise exception using errcode = '22023', message = 'invalid_reservation_receipt';
    end;
    if jsonb_typeof(parsed_receipt) <> 'object'
      or parsed_receipt ->> 'reservation_operation_id' <> p_reservation_operation_id::text
      or parsed_receipt ->> 'outcome' <> p_outcome
    then
      raise exception using errcode = '22023', message = 'reservation_receipt_mismatch';
    end if;
  end if;
  if p_outcome = 'succeeded' and (
    p_runner_unit_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or p_failure_code is not null
    or parsed_receipt ->> 'runner_unit_id' <> p_runner_unit_id
  ) then
    raise exception using errcode = '22023', message = 'invalid_reservation_result';
  end if;
  if p_outcome = 'terminal_failed' and (
    p_runner_unit_id is not null
    or p_failure_code !~ '^[a-z][a-z0-9_]{0,63}$'
    or parsed_receipt ->> 'failure_code' <> p_failure_code
  ) then
    raise exception using errcode = '22023', message = 'invalid_reservation_result';
  end if;

  select reservations.* into reservation_row
  from platform_store.house_runner_reservations reservations
  where reservations.reservation_operation_id = p_reservation_operation_id
  for update;
  if not found then
    return null;
  end if;
  if reservation_row.state in ('succeeded', 'terminal_failed') then
    if reservation_row.state = p_outcome
      and reservation_row.runner_unit_id is not distinct from p_runner_unit_id
      and reservation_row.reservation_receipt is not distinct from p_reservation_receipt
      and reservation_row.reservation_receipt_digest
        is not distinct from p_reservation_receipt_digest
      and reservation_row.failure_code is not distinct from p_failure_code
    then
      return platform_store.house_fill_operation_json_v1(reservation_row.launch_request_id);
    end if;
    raise exception using errcode = '23505', message = 'reservation_result_conflict';
  end if;

  update platform_store.house_runner_reservations reservations
  set state = p_outcome,
      runner_unit_id = p_runner_unit_id,
      reservation_receipt = p_reservation_receipt,
      reservation_receipt_digest = p_reservation_receipt_digest,
      failure_code = p_failure_code,
      last_transition_at = sampled_at
  where reservations.reservation_operation_id = p_reservation_operation_id;

  if p_outcome = 'terminal_failed' then
    perform platform_store.fail_house_fill_v1(
      reservation_row.house_fill_operation_id,
      'runner_reservation_failed'
    );
  end if;
  return platform_store.house_fill_operation_json_v1(reservation_row.launch_request_id);
end;
$$;

create function platform_api.complete_house_fill_v1(p_launch_request_id uuid)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  operation_row platform_store.house_fill_operations%rowtype;
  sampled_at timestamptz := clock_timestamp();
begin
  select operations.* into operation_row
  from platform_store.house_fill_operations operations
  where operations.launch_request_id = p_launch_request_id
  for update;
  if not found then
    return null;
  end if;
  if operation_row.state in ('assignments_complete', 'failed_pre_genesis') then
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if operation_row.state <> 'reserving'
    or exists (
      select 1 from platform_store.house_runner_reservations reservations
      where reservations.house_fill_operation_id = operation_row.house_fill_operation_id
        and reservations.state <> 'succeeded'
    )
    or (
      select count(*)
      from platform_store.house_runner_reservations reservations
      where reservations.house_fill_operation_id = operation_row.house_fill_operation_id
    ) <> operation_row.selected_assignment_count
  then
    raise exception using errcode = '55000', message = 'house_fill_reservations_incomplete';
  end if;

  insert into platform_store.house_agent_assignments (
    launch_request_id,
    house_fill_operation_id,
    listing_revision_digest,
    seat_id,
    setup_principal_reference,
    house_agent_revision_digest,
    model_slug,
    provider_slug,
    behavior_policy_id,
    behavior_policy_revision,
    agent_profile_id,
    agent_profile_revision,
    runner_template_id,
    runner_template_revision,
    accounting_tokenizer_id,
    accounting_tokenizer_revision,
    execution_allowance,
    selection_evidence_digest,
    reservation_operation_id,
    reservation_receipt_digest,
    assigned_at
  )
  select
    p_launch_request_id,
    operation_row.house_fill_operation_id,
    operation_row.listing_revision_digest,
    reservations.seat_id,
    'house:' || p_launch_request_id::text || ':' || reservations.seat_id,
    revisions.house_agent_revision_digest,
    revisions.model_slug,
    revisions.provider_slug,
    revisions.behavior_policy_id,
    revisions.behavior_policy_revision,
    revisions.agent_profile_id,
    revisions.agent_profile_revision,
    revisions.runner_template_id,
    revisions.runner_template_revision,
    revisions.accounting_tokenizer_id,
    revisions.accounting_tokenizer_revision,
    revisions.execution_allowance,
    operation_row.candidate_set_digest,
    reservations.reservation_operation_id,
    reservations.reservation_receipt_digest,
    sampled_at
  from platform_store.house_runner_reservations reservations
  join platform_store.house_agent_revisions revisions
    on revisions.house_agent_revision_digest = reservations.house_agent_revision_digest
  where reservations.house_fill_operation_id = operation_row.house_fill_operation_id
    and reservations.state = 'succeeded'
  order by reservations.seat_id;

  if (select count(*) from platform_store.house_agent_assignments assignments
      where assignments.house_fill_operation_id = operation_row.house_fill_operation_id)
     <> operation_row.selected_assignment_count
  then
    raise exception using errcode = '55000', message = 'house_fill_assignment_incomplete';
  end if;
  update platform_store.house_fill_operations operations
  set state = 'assignments_complete', completed_at = sampled_at
  where operations.house_fill_operation_id = operation_row.house_fill_operation_id;
  return platform_store.house_fill_operation_json_v1(p_launch_request_id);
end;
$$;

create function platform_api.read_house_fill_v1(
  p_requesting_account_id uuid,
  p_launch_request_id uuid
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
begin
  if not exists (
    select 1
    from platform_store.launch_requests launches
    where launches.launch_request_id = p_launch_request_id
      and (
        launches.creator_account_id = p_requesting_account_id
        or exists (
          select 1 from platform_store.seat_claims claims
          where claims.launch_request_id = p_launch_request_id
            and claims.controlling_account_id = p_requesting_account_id
            and claims.released_at is null
        )
      )
  ) then
    return null;
  end if;
  return platform_store.house_fill_operation_json_v1(p_launch_request_id);
end;
$$;

revoke all on platform_store.house_agent_revisions
  from public, anon, authenticated, service_role;
revoke all on platform_store.house_agent_host_approvals
  from public, anon, authenticated, service_role;
revoke all on platform_store.house_runner_capacity_gates
  from public, anon, authenticated, service_role;
revoke all on platform_store.house_fill_operations
  from public, anon, authenticated, service_role;
revoke all on platform_store.house_fill_candidate_evidence
  from public, anon, authenticated, service_role;
revoke all on platform_store.house_runner_reservations
  from public, anon, authenticated, service_role;
revoke all on platform_store.house_agent_assignments
  from public, anon, authenticated, service_role;

revoke execute on function platform_store.protect_house_agent_host_approval_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_house_fill_operation_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_house_fill_candidate_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_house_runner_reservation_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.house_fill_claims_closed_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.reject_closed_house_fill_claim_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.house_fill_operation_json_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.fail_house_fill_v1(
  uuid, text, text, bytea, bytea, integer
)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.start_house_fill_v1(uuid, uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.retain_house_fill_selection_v1(uuid, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_house_runner_reservation_v1(
  uuid, text, text, bytea, bytea, text
) from public, anon, authenticated, service_role;
revoke execute on function platform_api.complete_house_fill_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_house_fill_v1(uuid, uuid)
  from public, anon, authenticated, service_role;

grant select, insert on platform_store.house_agent_revisions to service_role;
grant select, insert on platform_store.house_agent_host_approvals to service_role;
grant update (available_for_new_assignments, availability_checked_at, revoked_at)
  on platform_store.house_agent_host_approvals to service_role;
grant select on platform_store.house_runner_capacity_gates to service_role;
grant update (created_at) on platform_store.house_runner_capacity_gates to service_role;
grant select, insert on platform_store.house_fill_operations to service_role;
grant update (
  state,
  host_installation_id,
  selection_retained_at,
  candidate_set_digest,
  exclusion_set_digest,
  selected_assignment_count,
  completed_at,
  failure_code
) on platform_store.house_fill_operations to service_role;
grant select, insert on platform_store.house_fill_candidate_evidence to service_role;
grant update (selected_seat_id, selection_rank)
  on platform_store.house_fill_candidate_evidence to service_role;
grant select, insert on platform_store.house_runner_reservations to service_role;
grant update (
  state,
  runner_unit_id,
  reservation_receipt,
  reservation_receipt_digest,
  failure_code,
  last_transition_at,
  released_at,
  release_receipt_digest
) on platform_store.house_runner_reservations to service_role;
grant select, insert on platform_store.house_agent_assignments to service_role;

grant execute on function platform_store.house_fill_claims_closed_v1(uuid)
  to service_role;
grant execute on function platform_store.house_fill_operation_json_v1(uuid)
  to service_role;
grant execute on function platform_store.fail_house_fill_v1(
  uuid, text, text, bytea, bytea, integer
)
  to service_role;
grant execute on function platform_api.start_house_fill_v1(uuid, uuid)
  to service_role;
grant execute on function platform_api.retain_house_fill_selection_v1(uuid, text)
  to service_role;
grant execute on function platform_api.record_house_runner_reservation_v1(
  uuid, text, text, bytea, bytea, text
) to service_role;
grant execute on function platform_api.complete_house_fill_v1(uuid)
  to service_role;
grant execute on function platform_api.read_house_fill_v1(uuid, uuid)
  to service_role;
