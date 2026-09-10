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
  false
),
(
  'hosted-dev',
  'blake3:134c19dbbd0b80bf2af98d095c8feca5026a00137c1077c16ccea5de95100ce4',
  'blake3:5555555555555555555555555555555555555555555555555555555555555555',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:2', 'utf8'), 'sha256'),
  false
),
(
  'hosted-dev',
  'blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a',
  'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:1', 'utf8'), 'sha256'),
  false
),
(
  'hosted-dev',
  'blake3:1a5edc340f6a2e7d66db5710d069dcf3c849028147f00fa2d99db2cd30ca9399',
  'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'blake3:8888888888888888888888888888888888888888888888888888888888888888',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:9', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:6146f54874747d6d4b16e06bca7872696f09c16dfe3b3177e5ab0c6c06034155',
  'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'blake3:8888888888888888888888888888888888888888888888888888888888888888',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:8', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:1665aa7c527861012829c237b1fdab760cbce918036a128d10ba06b7fa4cd098',
  'blake3:8888888888888888888888888888888888888888888888888888888888888888',
  'blake3:7777777777777777777777777777777777777777777777777777777777777777',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:8', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:5a826962c0f09c40a1b760d2c0eec9af216b9eb703b2e6f971fc24e32f0564d1',
  'blake3:7777777777777777777777777777777777777777777777777777777777777777',
  'blake3:7777777777777777777777777777777777777777777777777777777777777777',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:7', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:f61c494b644621e55eacf32dafbbf61f9471160eb2b91181e21c03ef5ed82271',
  'blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:10', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:c794a69db2c617724483aaa86e0b55f1d694936c4735932b75c2ae5cacdcfecd',
  'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
  'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:9', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:269f18e75fc3fb87b6f17e85080a0c8078f83116b74509b832d23c047ebcd316',
  'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
  'blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
  'blake3:913cd093b4b0752dba259ff0fdd90a5faac9da405adf2d7dd75e9716cc25a272',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:11', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:49bc5c43a57eb90f5a5aaa4f8118efba1f8ff00204d3ed45a511050928f0bc9f',
  'blake3:abababababababababababababababababababababababababababababababab',
  'blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
  'blake3:913cd093b4b0752dba259ff0fdd90a5faac9da405adf2d7dd75e9716cc25a272',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:10', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:3970a3d67cd15b55f01aa59013fb0d26e2b9ca7b4be635e6f659171b24aaed02',
  'blake3:1212121212121212121212121212121212121212121212121212121212121212',
  'blake3:1313131313131313131313131313131313131313131313131313131313131313',
  'blake3:8d06ab48b3a7ac17a68e1923b2227d69fe97291cb7f80ef3d234e8e43b483ec4',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:12', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:f406af7f13dff7f1ed41f48b469f10a68cd36d7b183f859211d2022bf1c72bb0',
  'blake3:1111111111111111111111111111111111111111111111111111111111111111',
  'blake3:1313131313131313131313131313131313131313131313131313131313131313',
  'blake3:8d06ab48b3a7ac17a68e1923b2227d69fe97291cb7f80ef3d234e8e43b483ec4',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:11', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:b624622cf68798968daa9548d32bddb3404298931fd532a8fd87266cc0ecf415',
  'blake3:1414141414141414141414141414141414141414141414141414141414141414',
  'blake3:1616161616161616161616161616161616161616161616161616161616161616',
  'blake3:d2328ad1da149d163ff7a5f2093b40ea0f232951cd34a5b24112d64cdfa837dd',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:13', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:742801ab85a5932827cf539af62547eff4f1041cf5c81aee1d39e2b492a02451',
  'blake3:1515151515151515151515151515151515151515151515151515151515151515',
  'blake3:1616161616161616161616161616161616161616161616161616161616161616',
  'blake3:d2328ad1da149d163ff7a5f2093b40ea0f232951cd34a5b24112d64cdfa837dd',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:12', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:bb9c56ffe925a130fe64c61386ca9f3d0638967719cc3dfed98d6ba06eead7fe',
  'blake3:1717171717171717171717171717171717171717171717171717171717171717',
  'blake3:1919191919191919191919191919191919191919191919191919191919191919',
  'blake3:a0e28c02e7b08a1f8621e04869a5806a46490b1d93ca1e74395dd8990cc12a64',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:14', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:5f718a17c4de50e72c67e441dec37c628582b2bf6d6bad2a6e10b6ef356a4a4d',
  'blake3:1818181818181818181818181818181818181818181818181818181818181818',
  'blake3:1919191919191919191919191919191919191919191919191919191919191919',
  'blake3:a0e28c02e7b08a1f8621e04869a5806a46490b1d93ca1e74395dd8990cc12a64',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:13', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:57c1e72a4147df339b35b557b35b1f2fa8239a66850c3d269df20ae111e2ea82',
  'blake3:2020202020202020202020202020202020202020202020202020202020202020',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'blake3:a0e28c02e7b08a1f8621e04869a5806a46490b1d93ca1e74395dd8990cc12a64',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:15', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:8ffb7a9247cc647565c7b7a0f3ed29301b15b450e8110f4c96b521fdc3883c7c',
  'blake3:2121212121212121212121212121212121212121212121212121212121212121',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'blake3:a0e28c02e7b08a1f8621e04869a5806a46490b1d93ca1e74395dd8990cc12a64',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:14', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:9788fe46953cf5c049c2dc457dac4dc5f916c45627457d4647c7e6d16c9308b9',
  'blake3:2323232323232323232323232323232323232323232323232323232323232323',
  'blake3:2525252525252525252525252525252525252525252525252525252525252525',
  'blake3:2626262626262626262626262626262626262626262626262626262626262626',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:cooperative-planner:16', 'utf8'), 'sha256'),
  true
),
(
  'hosted-dev',
  'blake3:2ec02c67644b04dcdcfcd76c9bd05e56cc4a4bf3e84b549be705fb26e43d5117',
  'blake3:2424242424242424242424242424242424242424242424242424242424242424',
  'blake3:2525252525252525252525252525252525252525252525252525252525252525',
  'blake3:2626262626262626262626262626262626262626262626262626262626262626',
  'openrouter-house',
  extensions.digest(convert_to('hosted-dev:skeptical-auditor:16', 'utf8'), 'sha256'),
  true
)
on conflict (host_installation_id, house_agent_revision_digest) do nothing;

-- These synthetic approvals are for the acknowledged local fake provider only.
-- Retain old identities, but select only the current reviewed Heist strategies.
update platform_store.house_agent_host_approvals
set available_for_new_assignments = false, availability_checked_at = clock_timestamp()
where host_installation_id = 'hosted-dev'
  and house_agent_revision_digest not in (
    'blake3:9788fe46953cf5c049c2dc457dac4dc5f916c45627457d4647c7e6d16c9308b9',
    'blake3:2ec02c67644b04dcdcfcd76c9bd05e56cc4a4bf3e84b549be705fb26e43d5117'
  )
  and available_for_new_assignments;

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
  ('00000000-0000-4000-8000-00000000d002'),
  -- Dedicated hosted qualification identities. Keep these separate from the
  -- retained manual-development identities above so old local setups do not
  -- consume the qualification account's single pre-Genesis reservation.
  ('00000000-0000-4000-8000-00000000d003'),
  ('00000000-0000-4000-8000-00000000d004')
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
  where identities.auth_user_id = '00000000-0000-4000-8000-00000000d003';

  if not found then
    perform * from platform_api.sync_github_identity_v1(
      '00000000-0000-4000-8000-00000000d003',
      'worldstream-development-qualification',
      'worldstream-local-qualification',
      null
    );
  elsif existing.provider_subject <> 'worldstream-development-qualification'
    or existing.github_login <> 'worldstream-local-qualification'
    or existing.erasure_requested_at is not null
  then
    raise exception using errcode = '23505', message = 'development_qualification_identity_conflict';
  end if;
end;
$$;

do $$
declare
  existing platform_store.github_identities%rowtype;
begin
  select * into existing
  from platform_store.github_identities identities
  where identities.auth_user_id = '00000000-0000-4000-8000-00000000d004';

  if not found then
    perform * from platform_api.sync_github_identity_v1(
      '00000000-0000-4000-8000-00000000d004',
      'worldstream-development-qualification-agent',
      'worldstream-local-qualification-agent',
      null
    );
  elsif existing.provider_subject <> 'worldstream-development-qualification-agent'
    or existing.github_login <> 'worldstream-local-qualification-agent'
    or existing.erasure_requested_at is not null
  then
    raise exception using errcode = '23505', message = 'development_qualification_agent_identity_conflict';
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
