-- Operational coordination for the single-authority hobby preview. This state
-- is intentionally small: WorldStream remains the Room authority while
-- Supabase records whether new platform work may be admitted and which paired
-- deployment/checkpoint evidence an operator has verified.

create table platform_store.hosted_operating_state (
  installation_id text primary key,
  launches_open boolean not null,
  house_fill_open boolean not null,
  maintenance_mode boolean not null,
  recovery_fenced boolean not null,
  generation bigint not null default 0,
  updated_at timestamptz not null default clock_timestamp(),
  constraint hosted_operating_installation_shape check (
    installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
  ),
  constraint hosted_operating_generation_bounds check (
    generation between 0 and 9007199254740991
  ),
  constraint hosted_operating_closed_during_maintenance check (
    not (maintenance_mode or recovery_fenced)
    or (not launches_open and not house_fill_open)
  )
);

insert into platform_store.hosted_operating_state (
  installation_id,
  launches_open,
  house_fill_open,
  maintenance_mode,
  recovery_fenced
) values ('fly-primary', true, true, false, false);

create table platform_store.hosted_deployment_revisions (
  deployment_revision_digest bytea primary key,
  canonical_document bytea not null,
  source_revision text not null,
  image_digest text not null,
  supabase_migration_head text not null,
  listing_revision_digests jsonb not null,
  recorded_at timestamptz not null default clock_timestamp(),
  constraint hosted_deployment_digest_shape check (
    octet_length(deployment_revision_digest) = 32
    and deployment_revision_digest = extensions.digest(canonical_document, 'sha256')
  ),
  constraint hosted_deployment_document_bounds check (
    octet_length(canonical_document) between 2 and 65536
  ),
  constraint hosted_deployment_source_shape check (
    source_revision ~ '^[0-9a-f]{40}([0-9a-f]{24})?$'
  ),
  constraint hosted_deployment_image_shape check (
    image_digest ~ '^sha256:[0-9a-f]{64}$'
  ),
  constraint hosted_deployment_migration_shape check (
    supabase_migration_head ~ '^[0-9]{14}$'
  ),
  constraint hosted_deployment_listing_shape check (
    jsonb_typeof(listing_revision_digests) = 'array'
    and jsonb_array_length(listing_revision_digests) between 1 and 64
  )
);

create table platform_store.hosted_recovery_checkpoint_events (
  checkpoint_id uuid not null,
  event_kind text not null,
  deployment_revision_digest bytea not null
    references platform_store.hosted_deployment_revisions(deployment_revision_digest)
    on delete restrict,
  manifest_digest bytea not null,
  worldstream_backup_digest bytea not null,
  controller_archive_digest bytea not null,
  supabase_dump_digest bytea not null,
  recorded_at timestamptz not null default clock_timestamp(),
  primary key (checkpoint_id, event_kind),
  constraint hosted_checkpoint_event_kind check (
    event_kind in ('created', 'isolated_restore_verified', 'recovery_cutover')
  ),
  constraint hosted_checkpoint_digest_shapes check (
    octet_length(manifest_digest) = 32
    and octet_length(worldstream_backup_digest) = 32
    and octet_length(controller_archive_digest) = 32
    and octet_length(supabase_dump_digest) = 32
  )
);

alter table platform_store.hosted_operating_state enable row level security;
alter table platform_store.hosted_deployment_revisions enable row level security;
alter table platform_store.hosted_recovery_checkpoint_events enable row level security;

create function platform_store.protect_hosted_operating_state_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE' then
    raise exception using errcode = '23000', message = 'invalid_hosted_operating_transition';
  end if;
  if new.installation_id <> old.installation_id
    or new.generation <> old.generation + 1
    or new.updated_at <= old.updated_at
  then
    raise exception using errcode = '23000', message = 'invalid_hosted_operating_transition';
  end if;
  return new;
end;
$$;

create trigger protect_hosted_operating_state_v1
before update or delete on platform_store.hosted_operating_state
for each row execute function platform_store.protect_hosted_operating_state_v1();

create function platform_store.reject_hosted_evidence_mutation_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  raise exception using errcode = '23000', message = 'immutable_hosted_evidence';
end;
$$;

create trigger protect_hosted_deployment_revision_v1
before update or delete on platform_store.hosted_deployment_revisions
for each row execute function platform_store.reject_hosted_evidence_mutation_v1();

create trigger protect_hosted_recovery_checkpoint_v1
before update or delete on platform_store.hosted_recovery_checkpoint_events
for each row execute function platform_store.reject_hosted_evidence_mutation_v1();

create function platform_store.require_hosted_launch_admission_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if not coalesce((
    select state.launches_open
    from platform_store.hosted_operating_state state
    where state.installation_id = 'fly-primary'
  ), false) then
    raise exception using errcode = '55000', message = 'hosted_launches_closed';
  end if;
  return new;
end;
$$;

create trigger require_hosted_launch_admission_v1
before insert on platform_store.launch_requests
for each row execute function platform_store.require_hosted_launch_admission_v1();

create function platform_store.require_hosted_house_fill_admission_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if not coalesce((
    select state.house_fill_open
    from platform_store.hosted_operating_state state
    where state.installation_id = 'fly-primary'
  ), false) then
    raise exception using errcode = '55000', message = 'hosted_house_fill_closed';
  end if;
  return new;
end;
$$;

create trigger require_hosted_house_fill_admission_v1
before insert on platform_store.house_fill_operations
for each row execute function platform_store.require_hosted_house_fill_admission_v1();

create function platform_api.read_hosted_operating_state_v1(
  p_installation_id text
)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select jsonb_build_object(
    'version', 'platform_hosted_operating_state.v1',
    'installation_id', state.installation_id,
    'launches_open', state.launches_open,
    'house_fill_open', state.house_fill_open,
    'maintenance_mode', state.maintenance_mode,
    'recovery_fenced', state.recovery_fenced,
    'generation', state.generation,
    'updated_at', state.updated_at
  )
  from platform_store.hosted_operating_state state
  where state.installation_id = p_installation_id;
$$;

create function platform_api.set_hosted_operating_state_v1(
  p_installation_id text,
  p_launches_open boolean,
  p_house_fill_open boolean,
  p_maintenance_mode boolean,
  p_recovery_fenced boolean
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  retained platform_store.hosted_operating_state%rowtype;
begin
  if p_installation_id is null
    or p_installation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or p_launches_open is null
    or p_house_fill_open is null
    or p_maintenance_mode is null
    or p_recovery_fenced is null
    or ((p_maintenance_mode or p_recovery_fenced)
      and (p_launches_open or p_house_fill_open))
  then
    raise exception using errcode = '22023', message = 'invalid_hosted_operating_state';
  end if;
  update platform_store.hosted_operating_state state
  set launches_open = p_launches_open,
      house_fill_open = p_house_fill_open,
      maintenance_mode = p_maintenance_mode,
      recovery_fenced = p_recovery_fenced,
      generation = state.generation + 1,
      updated_at = clock_timestamp()
  where state.installation_id = p_installation_id
  returning state.* into retained;
  if not found then
    raise exception using errcode = '55000', message = 'hosted_installation_unavailable';
  end if;
  return jsonb_build_object(
    'version', 'platform_hosted_operating_state.v1',
    'installation_id', retained.installation_id,
    'launches_open', retained.launches_open,
    'house_fill_open', retained.house_fill_open,
    'maintenance_mode', retained.maintenance_mode,
    'recovery_fenced', retained.recovery_fenced,
    'generation', retained.generation,
    'updated_at', retained.updated_at
  );
end;
$$;

create function platform_api.record_hosted_deployment_revision_v1(
  p_deployment_revision_digest bytea,
  p_canonical_document bytea,
  p_source_revision text,
  p_image_digest text,
  p_supabase_migration_head text,
  p_listing_revision_digests jsonb
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  existing platform_store.hosted_deployment_revisions%rowtype;
  inserted boolean := false;
begin
  if p_deployment_revision_digest is null
    or p_canonical_document is null
    or p_source_revision is null
    or p_image_digest is null
    or p_supabase_migration_head is null
    or p_listing_revision_digests is null
  then
    raise exception using errcode = '22023', message = 'invalid_hosted_deployment_revision';
  end if;
  if jsonb_typeof(p_listing_revision_digests) <> 'array'
    or jsonb_array_length(p_listing_revision_digests) not between 1 and 64
  then
    raise exception using errcode = '22023', message = 'invalid_hosted_deployment_revision';
  end if;
  if exists (
      select 1
      from jsonb_array_elements_text(p_listing_revision_digests) entries(value)
      where entries.value !~ '^blake3:[0-9a-f]{64}$'
    )
  then
    raise exception using errcode = '22023', message = 'invalid_hosted_deployment_revision';
  end if;
  select revisions.* into existing
  from platform_store.hosted_deployment_revisions revisions
  where revisions.deployment_revision_digest = p_deployment_revision_digest;
  if found then
    if existing.canonical_document <> p_canonical_document
      or existing.source_revision <> p_source_revision
      or existing.image_digest <> p_image_digest
      or existing.supabase_migration_head <> p_supabase_migration_head
      or existing.listing_revision_digests <> p_listing_revision_digests
    then
      raise exception using errcode = '23505', message = 'hosted_deployment_revision_conflict';
    end if;
  else
    insert into platform_store.hosted_deployment_revisions (
      deployment_revision_digest,
      canonical_document,
      source_revision,
      image_digest,
      supabase_migration_head,
      listing_revision_digests
    ) values (
      p_deployment_revision_digest,
      p_canonical_document,
      p_source_revision,
      p_image_digest,
      p_supabase_migration_head,
      p_listing_revision_digests
    );
    inserted := true;
  end if;
  return jsonb_build_object(
    'version', 'platform_hosted_deployment_recorded.v1',
    'deployment_revision_digest', 'sha256:' || encode(p_deployment_revision_digest, 'hex'),
    'was_created', inserted
  );
end;
$$;

create function platform_api.record_hosted_recovery_checkpoint_v1(
  p_checkpoint_id uuid,
  p_event_kind text,
  p_deployment_revision_digest bytea,
  p_manifest_digest bytea,
  p_worldstream_backup_digest bytea,
  p_controller_archive_digest bytea,
  p_supabase_dump_digest bytea
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  existing platform_store.hosted_recovery_checkpoint_events%rowtype;
  inserted boolean := false;
begin
  if p_checkpoint_id is null
    or p_event_kind not in ('created', 'isolated_restore_verified', 'recovery_cutover')
    or p_deployment_revision_digest is null
    or octet_length(p_deployment_revision_digest) <> 32
    or p_manifest_digest is null
    or octet_length(p_manifest_digest) <> 32
    or p_worldstream_backup_digest is null
    or octet_length(p_worldstream_backup_digest) <> 32
    or p_controller_archive_digest is null
    or octet_length(p_controller_archive_digest) <> 32
    or p_supabase_dump_digest is null
    or octet_length(p_supabase_dump_digest) <> 32
  then
    raise exception using errcode = '22023', message = 'invalid_hosted_checkpoint_event';
  end if;
  select events.* into existing
  from platform_store.hosted_recovery_checkpoint_events events
  where events.checkpoint_id = p_checkpoint_id
    and events.event_kind = p_event_kind;
  if found then
    if existing.deployment_revision_digest <> p_deployment_revision_digest
      or existing.manifest_digest <> p_manifest_digest
      or existing.worldstream_backup_digest <> p_worldstream_backup_digest
      or existing.controller_archive_digest <> p_controller_archive_digest
      or existing.supabase_dump_digest <> p_supabase_dump_digest
    then
      raise exception using errcode = '23505', message = 'hosted_checkpoint_event_conflict';
    end if;
  else
    insert into platform_store.hosted_recovery_checkpoint_events (
      checkpoint_id,
      event_kind,
      deployment_revision_digest,
      manifest_digest,
      worldstream_backup_digest,
      controller_archive_digest,
      supabase_dump_digest
    ) values (
      p_checkpoint_id,
      p_event_kind,
      p_deployment_revision_digest,
      p_manifest_digest,
      p_worldstream_backup_digest,
      p_controller_archive_digest,
      p_supabase_dump_digest
    );
    inserted := true;
  end if;
  return jsonb_build_object(
    'version', 'platform_hosted_checkpoint_recorded.v1',
    'checkpoint_id', p_checkpoint_id,
    'event_kind', p_event_kind,
    'was_created', inserted
  );
end;
$$;

revoke all on platform_store.hosted_operating_state
  from public, anon, authenticated, service_role;
revoke all on platform_store.hosted_deployment_revisions
  from public, anon, authenticated, service_role;
revoke all on platform_store.hosted_recovery_checkpoint_events
  from public, anon, authenticated, service_role;

revoke execute on function platform_store.protect_hosted_operating_state_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.reject_hosted_evidence_mutation_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.require_hosted_launch_admission_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.require_hosted_house_fill_admission_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_hosted_operating_state_v1(text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.set_hosted_operating_state_v1(
  text, boolean, boolean, boolean, boolean
) from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_hosted_deployment_revision_v1(
  bytea, bytea, text, text, text, jsonb
) from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_hosted_recovery_checkpoint_v1(
  uuid, text, bytea, bytea, bytea, bytea, bytea
) from public, anon, authenticated, service_role;

grant select, update on platform_store.hosted_operating_state to service_role;
grant select, insert on platform_store.hosted_deployment_revisions to service_role;
grant select, insert on platform_store.hosted_recovery_checkpoint_events to service_role;
grant execute on function platform_api.read_hosted_operating_state_v1(text)
  to service_role;
grant execute on function platform_api.set_hosted_operating_state_v1(
  text, boolean, boolean, boolean, boolean
) to service_role;
grant execute on function platform_api.record_hosted_deployment_revision_v1(
  bytea, bytea, text, text, text, jsonb
) to service_role;
grant execute on function platform_api.record_hosted_recovery_checkpoint_v1(
  uuid, text, bytea, bytea, bytea, bytea, bytea
) to service_role;
