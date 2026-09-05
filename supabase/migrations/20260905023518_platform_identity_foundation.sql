create schema if not exists platform_store;
create schema if not exists platform_api;

revoke all on schema public from public, anon, authenticated, service_role;
revoke all on schema platform_store from public, anon, authenticated, service_role;
revoke all on schema platform_api from public, anon, authenticated, service_role;

alter default privileges in schema platform_store
  revoke all on tables from public, anon, authenticated, service_role;
alter default privileges in schema platform_store
  revoke all on sequences from public, anon, authenticated, service_role;
alter default privileges in schema platform_store
  revoke execute on functions from public, anon, authenticated, service_role;
alter default privileges in schema platform_api
  revoke all on tables from public, anon, authenticated, service_role;
alter default privileges in schema platform_api
  revoke all on sequences from public, anon, authenticated, service_role;
alter default privileges in schema platform_api
  revoke execute on functions from public, anon, authenticated, service_role;

create table platform_store.platform_accounts (
  account_id uuid primary key default gen_random_uuid(),
  created_at timestamptz not null default clock_timestamp(),
  erased_at timestamptz,
  constraint platform_accounts_erasure_order
    check (erased_at is null or erased_at >= created_at)
);

create table platform_store.github_identities (
  auth_user_id uuid primary key references auth.users(id) on delete cascade,
  account_id uuid not null unique
    references platform_store.platform_accounts(account_id) on delete restrict,
  provider text not null default 'github',
  provider_subject text not null,
  github_login text,
  avatar_url text,
  public_profile_enabled boolean not null default false,
  erasure_requested_at timestamptz,
  linked_at timestamptz not null default clock_timestamp(),
  refreshed_at timestamptz not null default clock_timestamp(),
  constraint github_identities_provider check (provider = 'github'),
  constraint github_identities_subject_bounds check (
    length(provider_subject) between 1 and 128
    and provider_subject !~ '[[:cntrl:]]'
  ),
  constraint github_identities_login_bounds check (
    github_login is null
    or (
      length(github_login) between 1 and 128
      and github_login !~ '[[:cntrl:]]'
    )
  ),
  constraint github_identities_avatar_bounds check (
    avatar_url is null
    or (
      length(avatar_url) between 1 and 2048
      and avatar_url like 'https://%'
      and avatar_url !~ '[[:cntrl:]]'
    )
  ),
  constraint github_identities_refresh_order check (refreshed_at >= linked_at),
  constraint github_identities_erasure_order check (
    erasure_requested_at is null or erasure_requested_at >= linked_at
  ),
  constraint github_identities_provider_subject unique (provider, provider_subject)
);

create table platform_store.oauth_login_attempts (
  attempt_id uuid primary key default gen_random_uuid(),
  state_digest bytea not null unique,
  encrypted_pkce_verifier bytea not null,
  browser_nonce_digest bytea not null,
  return_target text not null,
  created_at timestamptz not null default clock_timestamp(),
  expires_at timestamptz not null,
  consumed_at timestamptz,
  constraint oauth_state_sha256 check (octet_length(state_digest) = 32),
  constraint oauth_nonce_sha256 check (octet_length(browser_nonce_digest) = 32),
  constraint oauth_pkce_ciphertext_bounds check (
    octet_length(encrypted_pkce_verifier) between 29 and 2048
  ),
  constraint oauth_return_target_bounds check (
    length(return_target) between 1 and 512
    and return_target like '/%'
    and return_target not like '//%'
    and position('//' in return_target) = 0
    and return_target !~ '[[:cntrl:]]'
  ),
  constraint oauth_exact_lifetime check (
    expires_at = created_at + interval '10 minutes'
  ),
  constraint oauth_consumption_order check (
    consumed_at is null
    or (consumed_at >= created_at and consumed_at <= expires_at)
  )
);

create index oauth_login_attempts_expiry_idx
  on platform_store.oauth_login_attempts (expires_at, attempt_id)
  where consumed_at is null;

create table platform_store.oauth_start_rate_limit (
  singleton boolean primary key default true,
  window_started_at timestamptz not null default clock_timestamp(),
  accepted_count integer not null default 0,
  constraint oauth_start_rate_limit_singleton check (singleton),
  constraint oauth_start_rate_limit_count check (accepted_count between 0 and 120)
);

create table platform_store.account_mutation_rate_limits (
  auth_user_id uuid primary key references auth.users(id) on delete cascade,
  window_started_at timestamptz not null default clock_timestamp(),
  accepted_count integer not null default 0,
  constraint account_mutation_rate_limit_count check (accepted_count between 0 and 32)
);

alter table platform_store.platform_accounts enable row level security;
alter table platform_store.github_identities enable row level security;
alter table platform_store.oauth_login_attempts enable row level security;
alter table platform_store.oauth_start_rate_limit enable row level security;
alter table platform_store.account_mutation_rate_limits enable row level security;

create function platform_store.protect_platform_account_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if new.account_id <> old.account_id
    or new.created_at <> old.created_at
    or old.erased_at is not null and new.erased_at is distinct from old.erased_at
  then
    raise exception using errcode = '23000', message = 'immutable_platform_account';
  end if;
  return new;
end;
$$;

create trigger protect_platform_account_v1
before update on platform_store.platform_accounts
for each row execute function platform_store.protect_platform_account_v1();

create function platform_store.protect_github_identity_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if new.auth_user_id <> old.auth_user_id
    or new.account_id <> old.account_id
    or new.provider <> old.provider
    or new.provider_subject <> old.provider_subject
    or new.linked_at <> old.linked_at
    or new.refreshed_at < old.refreshed_at
    or old.erasure_requested_at is not null
       and new.erasure_requested_at is distinct from old.erasure_requested_at
  then
    raise exception using errcode = '23000', message = 'immutable_github_identity';
  end if;
  return new;
end;
$$;

create trigger protect_github_identity_v1
before update on platform_store.github_identities
for each row execute function platform_store.protect_github_identity_v1();

create function platform_store.protect_oauth_attempt_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE' then
    if old.consumed_at is null and old.expires_at > clock_timestamp() then
      raise exception using errcode = '23000', message = 'active_oauth_attempt';
    end if;
    return old;
  end if;
  if new.attempt_id <> old.attempt_id
    or new.state_digest <> old.state_digest
    or new.encrypted_pkce_verifier <> old.encrypted_pkce_verifier
    or new.browser_nonce_digest <> old.browser_nonce_digest
    or new.return_target <> old.return_target
    or new.created_at <> old.created_at
    or new.expires_at <> old.expires_at
    or old.consumed_at is not null
    or new.consumed_at is null
  then
    raise exception using errcode = '23000', message = 'immutable_oauth_attempt';
  end if;
  return new;
end;
$$;

create trigger protect_oauth_attempt_v1
before update or delete on platform_store.oauth_login_attempts
for each row execute function platform_store.protect_oauth_attempt_v1();

create function platform_store.admit_oauth_start_v1()
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  admitted boolean;
  sampled_at timestamptz := clock_timestamp();
begin
  insert into platform_store.oauth_start_rate_limit (
    singleton,
    window_started_at,
    accepted_count
  ) values (true, sampled_at, 1)
  on conflict (singleton) do update
  set window_started_at = case
        when platform_store.oauth_start_rate_limit.window_started_at
          <= excluded.window_started_at - interval '1 minute'
        then excluded.window_started_at
        else platform_store.oauth_start_rate_limit.window_started_at
      end,
      accepted_count = case
        when platform_store.oauth_start_rate_limit.window_started_at
          <= excluded.window_started_at - interval '1 minute'
        then 1
        else platform_store.oauth_start_rate_limit.accepted_count + 1
      end
  where platform_store.oauth_start_rate_limit.window_started_at
          <= excluded.window_started_at - interval '1 minute'
     or platform_store.oauth_start_rate_limit.accepted_count < 120
  returning true into admitted;
  return coalesce(admitted, false);
end;
$$;

create function platform_store.admit_account_mutation_v1(p_auth_user_id uuid)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  admitted boolean;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_auth_user_id is null then
    raise exception using errcode = '22023', message = 'invalid_auth_user';
  end if;
  insert into platform_store.account_mutation_rate_limits (
    auth_user_id,
    window_started_at,
    accepted_count
  ) values (p_auth_user_id, sampled_at, 1)
  on conflict (auth_user_id) do update
  set window_started_at = case
        when platform_store.account_mutation_rate_limits.window_started_at
          <= excluded.window_started_at - interval '1 minute'
        then excluded.window_started_at
        else platform_store.account_mutation_rate_limits.window_started_at
      end,
      accepted_count = case
        when platform_store.account_mutation_rate_limits.window_started_at
          <= excluded.window_started_at - interval '1 minute'
        then 1
        else platform_store.account_mutation_rate_limits.accepted_count + 1
      end
  where platform_store.account_mutation_rate_limits.window_started_at
          <= excluded.window_started_at - interval '1 minute'
     or platform_store.account_mutation_rate_limits.accepted_count < 32
  returning true into admitted;
  return coalesce(admitted, false);
end;
$$;

create function platform_api.begin_github_oauth_v1(
  p_state_digest bytea,
  p_encrypted_pkce_verifier bytea,
  p_browser_nonce_digest bytea,
  p_return_target text
)
returns table(attempt_id uuid, expires_at timestamptz)
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if p_state_digest is null
    or octet_length(p_state_digest) <> 32
    or p_encrypted_pkce_verifier is null
    or octet_length(p_encrypted_pkce_verifier) not between 29 and 2048
    or p_browser_nonce_digest is null
    or octet_length(p_browser_nonce_digest) <> 32
    or p_return_target is null
    or length(p_return_target) not between 1 and 512
    or p_return_target not like '/%'
    or p_return_target like '//%'
    or position('//' in p_return_target) <> 0
    or p_return_target ~ '[[:cntrl:]]'
  then
    raise exception using errcode = '23514', message = 'invalid_oauth_attempt';
  end if;
  if not platform_store.admit_oauth_start_v1() then
    return;
  end if;
  return query
    with timing as (select clock_timestamp() as now)
    insert into platform_store.oauth_login_attempts (
      state_digest,
      encrypted_pkce_verifier,
      browser_nonce_digest,
      return_target,
      created_at,
      expires_at
    ) select
      p_state_digest,
      p_encrypted_pkce_verifier,
      p_browser_nonce_digest,
      p_return_target,
      timing.now,
      timing.now + interval '10 minutes'
    from timing
    returning oauth_login_attempts.attempt_id, oauth_login_attempts.expires_at;
end;
$$;

create function platform_api.consume_github_oauth_v1(
  p_state_digest bytea,
  p_browser_nonce_digest bytea,
  p_return_target text
)
returns table(
  attempt_id uuid,
  encrypted_pkce_verifier bytea,
  return_target text
)
language sql
security invoker
set search_path = ''
as $$
  update platform_store.oauth_login_attempts
  set consumed_at = clock_timestamp()
  where state_digest = p_state_digest
    and browser_nonce_digest = p_browser_nonce_digest
    and return_target = p_return_target
    and consumed_at is null
    and expires_at >= clock_timestamp()
  returning
    oauth_login_attempts.attempt_id,
    oauth_login_attempts.encrypted_pkce_verifier,
    oauth_login_attempts.return_target;
$$;

create function platform_api.purge_expired_oauth_attempts_v1(p_limit integer default 100)
returns integer
language plpgsql
security invoker
set search_path = ''
as $$
declare
  deleted_count integer;
begin
  if p_limit < 1 or p_limit > 500 then
    raise exception using errcode = '22023', message = 'invalid_purge_limit';
  end if;
  with selected as (
    select attempt_id
    from platform_store.oauth_login_attempts
    where expires_at <= clock_timestamp() or consumed_at is not null
    order by expires_at, attempt_id
    limit p_limit
    for update skip locked
  ), deleted as (
    delete from platform_store.oauth_login_attempts attempts
    using selected
    where attempts.attempt_id = selected.attempt_id
    returning 1
  )
  select count(*)::integer into deleted_count from deleted;
  return deleted_count;
end;
$$;

create function platform_store.write_github_identity_v1(
  p_auth_user_id uuid,
  p_provider_subject text,
  p_github_login text,
  p_avatar_url text
)
returns table(
  account_id uuid,
  erased_at timestamptz,
  public_profile_enabled boolean
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  resolved_account_id uuid;
  existing_provider_subject text;
  subject_auth_user_id uuid;
begin
  if p_auth_user_id is null
    or p_provider_subject is null
    or length(p_provider_subject) not between 1 and 128
    or p_provider_subject ~ '[[:cntrl:]]'
    or p_github_login is not null and (
      length(p_github_login) not between 1 and 128
      or p_github_login ~ '[[:cntrl:]]'
    )
    or p_avatar_url is not null and (
      length(p_avatar_url) not between 1 and 2048
      or p_avatar_url not like 'https://%'
      or p_avatar_url ~ '[[:cntrl:]]'
    )
  then
    raise exception using errcode = '22023', message = 'invalid_github_identity';
  end if;

  perform pg_advisory_xact_lock(hashtextextended('github:' || p_provider_subject, 0));
  select identities.account_id, identities.provider_subject
    into resolved_account_id, existing_provider_subject
  from platform_store.github_identities identities
  where identities.auth_user_id = p_auth_user_id
  for update;

  if resolved_account_id is not null and existing_provider_subject <> p_provider_subject then
    raise exception using errcode = '23505', message = 'github_identity_conflict';
  end if;

  select identities.auth_user_id
    into subject_auth_user_id
  from platform_store.github_identities identities
  where identities.provider = 'github'
    and identities.provider_subject = p_provider_subject
  for update;

  if subject_auth_user_id is not null and subject_auth_user_id <> p_auth_user_id then
    raise exception using errcode = '23505', message = 'github_identity_conflict';
  end if;

  if resolved_account_id is null then
    insert into platform_store.platform_accounts default values
      returning platform_accounts.account_id into resolved_account_id;
    insert into platform_store.github_identities (
      auth_user_id,
      account_id,
      provider_subject,
      github_login,
      avatar_url
    ) values (
      p_auth_user_id,
      resolved_account_id,
      p_provider_subject,
      p_github_login,
      p_avatar_url
    );
  else
    if exists (
      select 1
      from platform_store.github_identities identities
      join platform_store.platform_accounts accounts
        on accounts.account_id = identities.account_id
      where identities.auth_user_id = p_auth_user_id
        and (identities.erasure_requested_at is not null or accounts.erased_at is not null)
    ) then
      raise exception using errcode = '55000', message = 'account_erasure_pending';
    end if;
    update platform_store.github_identities
    set github_login = p_github_login,
        avatar_url = p_avatar_url,
        refreshed_at = greatest(clock_timestamp(), refreshed_at)
    where auth_user_id = p_auth_user_id;
  end if;

  return query
  select accounts.account_id, accounts.erased_at, identities.public_profile_enabled
  from platform_store.platform_accounts accounts
  join platform_store.github_identities identities
    on identities.account_id = accounts.account_id
  where identities.auth_user_id = p_auth_user_id;
end;
$$;

create function platform_api.sync_github_identity_v1(
  p_auth_user_id uuid,
  p_provider_subject text
)
returns table(
  account_id uuid,
  erased_at timestamptz,
  public_profile_enabled boolean
)
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if p_auth_user_id is null
    or p_provider_subject is null
    or length(p_provider_subject) not between 1 and 128
    or p_provider_subject ~ '[[:cntrl:]]'
  then
    raise exception using errcode = '22023', message = 'invalid_github_identity';
  end if;
  return query
  select accounts.account_id, accounts.erased_at, identities.public_profile_enabled
  from platform_store.github_identities identities
  join platform_store.platform_accounts accounts
    on accounts.account_id = identities.account_id
  where identities.auth_user_id = p_auth_user_id
    and identities.provider = 'github'
    and identities.provider_subject = p_provider_subject;
end;
$$;

create function platform_api.sync_github_identity_v1(
  p_auth_user_id uuid,
  p_provider_subject text,
  p_github_login text,
  p_avatar_url text
)
returns table(
  account_id uuid,
  erased_at timestamptz,
  public_profile_enabled boolean
)
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if not platform_store.admit_account_mutation_v1(p_auth_user_id) then
    return;
  end if;
  return query
  select * from platform_store.write_github_identity_v1(
    p_auth_user_id,
    p_provider_subject,
    p_github_login,
    p_avatar_url
  );
end;
$$;

-- PostgREST overload for the session-refresh form of identity synchronization.
-- The required true literal selects durable admission without accepting or
-- rewriting presentation data from an older sealed browser session.
create function platform_api.sync_github_identity_v1(
  p_auth_user_id uuid,
  p_provider_subject text,
  p_admit_session_refresh boolean
)
returns table(
  account_id uuid,
  erased_at timestamptz,
  public_profile_enabled boolean,
  admitted boolean
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  resolved_account_id uuid;
  resolved_erased_at timestamptz;
  resolved_public_profile_enabled boolean;
  identity_erasure_requested_at timestamptz;
begin
  if p_admit_session_refresh is distinct from true
    or p_auth_user_id is null
    or p_provider_subject is null
    or length(p_provider_subject) not between 1 and 128
    or p_provider_subject ~ '[[:cntrl:]]'
  then
    raise exception using errcode = '22023', message = 'invalid_github_identity';
  end if;

  select
    accounts.account_id,
    accounts.erased_at,
    identities.public_profile_enabled,
    identities.erasure_requested_at
  into
    resolved_account_id,
    resolved_erased_at,
    resolved_public_profile_enabled,
    identity_erasure_requested_at
  from platform_store.github_identities identities
  join platform_store.platform_accounts accounts
    on accounts.account_id = identities.account_id
  where identities.auth_user_id = p_auth_user_id
    and identities.provider = 'github'
    and identities.provider_subject = p_provider_subject;

  if resolved_account_id is null then
    return;
  end if;

  if identity_erasure_requested_at is not null then
    resolved_erased_at := coalesce(resolved_erased_at, identity_erasure_requested_at);
  end if;

  return query select
    resolved_account_id,
    resolved_erased_at,
    resolved_public_profile_enabled,
    case
      when resolved_erased_at is not null then false
      else platform_store.admit_account_mutation_v1(p_auth_user_id)
    end;
end;
$$;

create function platform_api.set_public_profile_v1(
  p_auth_user_id uuid,
  p_enabled boolean
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if p_enabled is null then
    raise exception using errcode = '22023', message = 'invalid_profile_choice';
  end if;
  update platform_store.github_identities identities
  set public_profile_enabled = p_enabled,
      refreshed_at = greatest(clock_timestamp(), refreshed_at)
  from platform_store.platform_accounts accounts
  where identities.auth_user_id = p_auth_user_id
    and accounts.account_id = identities.account_id
    and identities.erasure_requested_at is null
    and accounts.erased_at is null;
  return found;
end;
$$;

create function platform_api.begin_account_erasure_v1(p_auth_user_id uuid)
returns table(account_id uuid, erasure_requested_at timestamptz)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  resolved_account_id uuid;
  requested_at timestamptz;
begin
  select identities.account_id
    into resolved_account_id
  from platform_store.github_identities identities
  where identities.auth_user_id = p_auth_user_id
  for update;
  if resolved_account_id is null then
    return;
  end if;

  perform 1
  from platform_store.platform_accounts accounts
  where accounts.account_id = resolved_account_id
  for update;

  requested_at := clock_timestamp();
  update platform_store.platform_accounts accounts
  set erased_at = coalesce(accounts.erased_at, requested_at)
  where accounts.account_id = resolved_account_id
  returning accounts.erased_at into requested_at;

  update platform_store.github_identities identities
  set erasure_requested_at = coalesce(identities.erasure_requested_at, requested_at),
      public_profile_enabled = false,
      github_login = null,
      avatar_url = null,
      refreshed_at = greatest(clock_timestamp(), identities.refreshed_at)
  where identities.auth_user_id = p_auth_user_id;

  return query select resolved_account_id, requested_at;
end;
$$;

revoke all on all tables in schema platform_store
  from public, anon, authenticated, service_role;
revoke all on all sequences in schema platform_store
  from public, anon, authenticated, service_role;
revoke execute on all functions in schema platform_store
  from public, anon, authenticated, service_role;
revoke execute on all functions in schema platform_api
  from public, anon, authenticated, service_role;

grant usage on schema platform_store to service_role;
grant usage on schema platform_api to service_role;
grant select, insert on platform_store.platform_accounts to service_role;
grant update (erased_at) on platform_store.platform_accounts to service_role;
grant select, insert on platform_store.github_identities to service_role;
grant update (
  github_login,
  avatar_url,
  public_profile_enabled,
  erasure_requested_at,
  refreshed_at
) on platform_store.github_identities to service_role;
grant select, insert, delete on platform_store.oauth_login_attempts to service_role;
grant update (consumed_at) on platform_store.oauth_login_attempts to service_role;
grant select, insert on platform_store.oauth_start_rate_limit to service_role;
grant update (window_started_at, accepted_count)
  on platform_store.oauth_start_rate_limit to service_role;
grant select, insert on platform_store.account_mutation_rate_limits to service_role;
grant update (window_started_at, accepted_count)
  on platform_store.account_mutation_rate_limits to service_role;

grant execute on function platform_store.admit_oauth_start_v1() to service_role;
grant execute on function platform_store.admit_account_mutation_v1(uuid) to service_role;
grant execute on function platform_store.write_github_identity_v1(uuid, text, text, text)
  to service_role;

grant execute on function platform_api.begin_github_oauth_v1(bytea, bytea, bytea, text)
  to service_role;
grant execute on function platform_api.consume_github_oauth_v1(bytea, bytea, text)
  to service_role;
grant execute on function platform_api.purge_expired_oauth_attempts_v1(integer)
  to service_role;
grant execute on function platform_api.sync_github_identity_v1(uuid, text)
  to service_role;
grant execute on function platform_api.sync_github_identity_v1(uuid, text, text, text)
  to service_role;
grant execute on function platform_api.sync_github_identity_v1(uuid, text, boolean)
  to service_role;
grant execute on function platform_api.set_public_profile_v1(uuid, boolean)
  to service_role;
grant execute on function platform_api.begin_account_erasure_v1(uuid)
  to service_role;
