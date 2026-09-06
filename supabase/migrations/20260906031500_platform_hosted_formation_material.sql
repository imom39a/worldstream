-- Private server-only material used to derive and resume one exact hosted
-- launch. The browser-facing BFF must project a smaller safe formation DTO.
create function platform_api.read_hosted_launch_material_v1(
  p_creator_account_id uuid,
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
  listing_row platform_store.activity_listing_revisions%rowtype;
  claims jsonb;
  assignments jsonb;
begin
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
    and launches.creator_account_id = p_creator_account_id;
  if not found then return null; end if;

  select listings.* into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  select coalesce(jsonb_agg(jsonb_build_object(
    'seat_id', claims.seat_id,
    'role', claims.role,
    'display_name', seats.value ->> 'display_name',
    'participation_kind', claims.participation_kind,
    'principal_reference', claims.principal_reference
  ) order by seats.ordinality), '[]'::jsonb)
  into claims
  from platform_store.seat_claims claims
  join jsonb_array_elements(listing_row.seat_templates)
       with ordinality seats(value, ordinality)
    on seats.value ->> 'seat_id' = claims.seat_id
  where claims.launch_request_id = p_launch_request_id
    and claims.released_at is null;

  select coalesce(jsonb_agg(jsonb_build_object(
    'house_agent_assignment_id', assignments.house_agent_assignment_id,
    'seat_id', assignments.seat_id,
    'principal_reference', assignments.setup_principal_reference,
    'display_name', revisions.display_name,
    'house_agent_revision_digest', assignments.house_agent_revision_digest,
    'agent_profile', jsonb_build_object(
      'profile_id', assignments.agent_profile_id,
      'revision', assignments.agent_profile_revision
    ),
    'runner_template', jsonb_build_object(
      'template_id', assignments.runner_template_id,
      'revision', assignments.runner_template_revision
    ),
    'reservation_receipt', convert_from(reservations.reservation_receipt, 'utf8')::jsonb
  ) order by assignments.seat_id), '[]'::jsonb)
  into assignments
  from platform_store.house_agent_assignments assignments
  join platform_store.house_agent_revisions revisions
    on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
  join platform_store.house_runner_reservations reservations
    on reservations.reservation_operation_id = assignments.reservation_operation_id
  where assignments.launch_request_id = p_launch_request_id
    and reservations.state = 'succeeded';

  return jsonb_build_object(
    'version', 'platform_hosted_launch_material.v1',
    'launch_request_id', launch_row.launch_request_id,
    'listing_revision_digest', launch_row.listing_revision_digest,
    'state', launch_row.state,
    'expires_at', launch_row.expires_at,
    'launch_inputs', convert_from(launch_row.canonical_launch_input, 'utf8')::jsonb,
    'house_fill_choice', launch_row.house_fill_choice,
    'creator_access_choice', launch_row.creator_access_choice,
    'creator_seat_id', launch_row.creator_seat_id,
    'host_installation_id', launch_row.host_installation_id,
    'room_setup_operation_id', launch_row.room_setup_operation_id,
    'roster_frozen', launch_row.roster_frozen_at is not null,
    'host_mutation_started', launch_row.host_mutation_started_at is not null,
    'claims', claims,
    'house_assignments', assignments
  );
end;
$$;

revoke execute on function platform_api.read_hosted_launch_material_v1(uuid, uuid)
  from public, anon, authenticated, service_role;
grant execute on function platform_api.read_hosted_launch_material_v1(uuid, uuid)
  to service_role;
