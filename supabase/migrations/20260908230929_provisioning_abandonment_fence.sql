-- Additive, repeat-safe follow-up for the provisioning-abandonment fence.
-- The preceding migration contains the base DDL for clean installs. Some
-- already-running local development histories recorded its timestamp before
-- that file was finalized, so this migration re-applies the same immutable
-- contract without rewriting migration history.
create table if not exists platform_store.provisioning_abandonments (
  launch_request_id uuid primary key references platform_store.launch_requests(launch_request_id) on delete restrict,
  abandonment_evidence_digest bytea not null check (octet_length(abandonment_evidence_digest) = 32),
  canonical_abandonment_evidence bytea not null check (octet_length(canonical_abandonment_evidence) between 2 and 8192),
  recorded_at timestamptz not null default clock_timestamp()
);
alter table platform_store.provisioning_abandonments enable row level security;
drop trigger if exists protect_provisioning_abandonment_v1 on platform_store.provisioning_abandonments;
create trigger protect_provisioning_abandonment_v1
before update or delete on platform_store.provisioning_abandonments
for each row execute function platform_store.reject_immutable_formation_row_v1();

create or replace function platform_api.record_provisioning_abandonment_v1(
  p_launch_request_id uuid,
  p_canonical_abandonment_evidence bytea,
  p_abandonment_evidence_digest bytea
) returns boolean
language plpgsql security invoker set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  existing platform_store.provisioning_abandonments%rowtype;
  evidence jsonb;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_launch_request_id is null
    or octet_length(p_canonical_abandonment_evidence) not between 2 and 8192
    or octet_length(p_abandonment_evidence_digest) <> 32
    or p_abandonment_evidence_digest <> extensions.digest(p_canonical_abandonment_evidence, 'sha256')
  then raise exception using errcode = '22023', message = 'invalid_provisioning_abandonment'; end if;
  begin evidence := convert_from(p_canonical_abandonment_evidence, 'utf8')::jsonb;
  exception when others then raise exception using errcode = '22023', message = 'invalid_provisioning_abandonment'; end;
  if jsonb_typeof(evidence) <> 'object'
    or not (evidence ?& array['schema','host_installation_id','listing_revision_digest','launch_request_id','launch_request_digest','room_setup_operation_id','genesis_committed','provisioning_fence_digest','authentication_tag'])
    or exists (select 1 from jsonb_object_keys(evidence) keys(key) where keys.key not in ('schema','host_installation_id','listing_revision_digest','launch_request_id','launch_request_digest','room_setup_operation_id','genesis_committed','provisioning_fence_digest','authentication_tag'))
    or evidence ->> 'schema' <> 'worldstream/hosted-provisioning-abandonment-evidence/v1'
    or evidence ->> 'host_installation_id' !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or evidence ->> 'listing_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'launch_request_id' !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
    or evidence ->> 'launch_request_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'room_setup_operation_id' !~ '^[a-z][a-z0-9-]{0,63}$'
    or evidence -> 'genesis_committed' <> 'false'::jsonb
    or evidence ->> 'provisioning_fence_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'authentication_tag' !~ '^[0-9a-f]{64}$'
  then raise exception using errcode = '22023', message = 'invalid_provisioning_abandonment'; end if;

  perform pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended('provisioning-abandonment:' || p_launch_request_id::text, 0));
  select launches.* into launch_row from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id for update;
  if not found then return false; end if;
  select abandoned.* into existing from platform_store.provisioning_abandonments abandoned
  where abandoned.launch_request_id = p_launch_request_id;
  if found then
    return existing.canonical_abandonment_evidence = p_canonical_abandonment_evidence
      and existing.abandonment_evidence_digest = p_abandonment_evidence_digest;
  end if;
  -- No scheduling/deadline path can reach this branch. The Host had already
  -- fsync'd an exact-operation no-Genesis fence before returning evidence.
  if launch_row.state not in ('provisioning', 'reconciling')
    or launch_row.roster_frozen_at is null or launch_row.host_mutation_started_at is null
    or exists (select 1 from platform_store.activity_runs runs where runs.launch_request_id = p_launch_request_id)
    or exists (select 1 from platform_store.reconciliation_receipts receipts where receipts.launch_request_id = p_launch_request_id and receipts.disposition = 'conflict')
    or evidence ->> 'host_installation_id' <> launch_row.host_installation_id
    or evidence ->> 'listing_revision_digest' <> launch_row.listing_revision_digest
    or evidence ->> 'launch_request_id' <> p_launch_request_id::text
    or evidence ->> 'room_setup_operation_id' <> launch_row.room_setup_operation_id
  then return false; end if;
  perform reservations.reservation_operation_id from platform_store.house_runner_reservations reservations
  where reservations.launch_request_id = p_launch_request_id and reservations.state = 'succeeded' and reservations.released_at is null
  order by reservations.reservation_operation_id for update;
  insert into platform_store.provisioning_abandonments (launch_request_id, abandonment_evidence_digest, canonical_abandonment_evidence, recorded_at)
  values (p_launch_request_id, p_abandonment_evidence_digest, p_canonical_abandonment_evidence, sampled_at);
  update platform_store.house_runner_reservations reservations
  set state = 'released', released_at = sampled_at, release_receipt_digest = p_abandonment_evidence_digest, last_transition_at = sampled_at
  where reservations.launch_request_id = p_launch_request_id and reservations.state = 'succeeded' and reservations.released_at is null;
  perform platform_store.release_launch_capacity_v1(p_launch_request_id, 'failed_pre_genesis');
  update platform_store.launch_requests launches set state = 'failed_pre_genesis', last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id and launches.state in ('provisioning', 'reconciling');
  return true;
end;
$$;

revoke all on platform_store.provisioning_abandonments from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_provisioning_abandonment_v1(uuid, bytea, bytea) from public, anon, authenticated, service_role;
grant select, insert on platform_store.provisioning_abandonments to service_role;
grant execute on function platform_api.record_provisioning_abandonment_v1(uuid, bytea, bytea) to service_role;
