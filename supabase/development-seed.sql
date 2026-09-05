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

insert into auth.users(id)
values ('00000000-0000-4000-8000-00000000d001')
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
