-- Recovery can reuse prior consent, never create it. The returned private
-- material is identical to the creator's frozen setup and stays server-only.
create function platform_api.read_hosted_recovery_material_v1(p_launch_request_id uuid)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select platform_api.read_hosted_launch_material_v1(launches.creator_account_id, launches.launch_request_id)
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
    and launches.host_mutation_started_at is not null
    and launches.roster_frozen_at is not null
    and launches.state in ('provisioning', 'reconciling', 'run_created')
    and not exists (
      select 1 from platform_store.reconciliation_receipts receipts
      where receipts.launch_request_id = launches.launch_request_id
        and receipts.disposition = 'conflict'
    );
$$;

revoke execute on function platform_api.read_hosted_recovery_material_v1(uuid)
  from public, anon, authenticated, service_role;
grant execute on function platform_api.read_hosted_recovery_material_v1(uuid) to service_role;

-- Observe the applied schema, not an operator-supplied environment label.
grant usage on schema supabase_migrations to service_role;
grant select (version) on supabase_migrations.schema_migrations to service_role;
create function platform_api.read_hosted_schema_head_v1()
returns text
language sql
stable
security invoker
set search_path = ''
as $$ select max(version) from supabase_migrations.schema_migrations; $$;
revoke execute on function platform_api.read_hosted_schema_head_v1()
  from public, anon, authenticated, service_role;
grant execute on function platform_api.read_hosted_schema_head_v1() to service_role;
