-- This file is not part of db.seed.sql_paths. The hosted:dev supervisor applies
-- it only after setting this exact connection-local acknowledgement.
do $$
begin
  if current_setting('worldstream.development_seed', true) is distinct from
    'visible-local-only'
  then
    raise exception using
      errcode = '42501',
      message = 'development_seed_acknowledgement_required';
  end if;
end;
$$;

insert into platform_store.house_agent_host_approvals (
  host_installation_id,
  house_agent_revision_digest,
  agent_profile_revision_digest,
  runner_template_revision_digest,
  runner_executable_digest,
  named_credential_reference,
  approval_receipt_digest,
  available_for_new_assignments
) values
(
  'hosted-dev',
  'blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81',
  'blake3:1111111111111111111111111111111111111111111111111111111111111111',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:1', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a',
  'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:1', 'utf8'), 'sha256'),
  true
)
on conflict (host_installation_id, house_agent_revision_digest) do nothing;

do $$
begin
  if (select count(*)
      from platform_store.house_agent_host_approvals approvals
      where approvals.host_installation_id = 'hosted-dev'
        and approvals.revoked_at is null
        and approvals.available_for_new_assignments) <> 2
  then
    raise exception using errcode = '23505', message = 'development_house_approval_conflict';
  end if;
end;
$$;

insert into auth.users(id)
values
  ('00000000-0000-4000-8000-00000000d001'),
  ('00000000-0000-4000-8000-00000000d002')
on conflict (id) do nothing;

do $$
declare
  existing platform_store.github_identities%rowtype;
begin
  select * into existing
  from platform_store.github_identities identities
  where identities.auth_user_id = '00000000-0000-4000-8000-00000000d001';

  if not found then
    perform * from platform_api.sync_github_identity_v1(
      '00000000-0000-4000-8000-00000000d001',
      'worldstream-development',
      'worldstream-local-developer',
      null
    );
  elsif existing.provider_subject <> 'worldstream-development'
    or existing.github_login <> 'worldstream-local-developer'
    or existing.erasure_requested_at is not null
  then
    raise exception using errcode = '23505', message = 'development_identity_conflict';
  end if;
end;
$$;

do $$
declare
  existing platform_store.github_identities%rowtype;
begin
  select * into existing
  from platform_store.github_identities identities
  where identities.auth_user_id = '00000000-0000-4000-8000-00000000d002';

  if not found then
    perform * from platform_api.sync_github_identity_v1(
      '00000000-0000-4000-8000-00000000d002',
      'worldstream-development-agent',
      'worldstream-local-browser-agent',
      null
    );
  elsif existing.provider_subject <> 'worldstream-development-agent'
    or existing.github_login <> 'worldstream-local-browser-agent'
    or existing.erasure_requested_at is not null
  then
    raise exception using errcode = '23505', message = 'development_agent_identity_conflict';
  end if;
end;
$$;
