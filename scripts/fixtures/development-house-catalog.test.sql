begin;
create extension if not exists pgtap with schema extensions;
set local search_path = public, extensions;
select plan(4);
set local worldstream.development_seed = 'visible-local-only';
\ir ../../supabase/development-seed.sql

select is((
  select count(distinct approvals.house_agent_revision_digest)::integer
  from platform_store.house_agent_host_approvals approvals
  join platform_store.activity_listing_revisions listings
    on listings.listing_revision_digest = 'blake3:3cdaaa7b2402b816ded0b36d5419f405b1be1428b37c89155a805d39bf826069'
  cross join lateral jsonb_array_elements(listings.seat_templates) seats(value)
  where approvals.host_installation_id = 'hosted-dev'
    and approvals.available_for_new_assignments and approvals.revoked_at is null
    and (seats.value -> 'allowed_house_agent_revisions') ? approvals.house_agent_revision_digest
), 2, 'local fake House pool supplies both current Listing strategies');

\ir ../../supabase/development-seed.sql
select is((select count(*)::integer from platform_store.house_agent_host_approvals
  where host_installation_id = 'hosted-dev' and available_for_new_assignments
    and revoked_at is null), 2, 'repeated local seed has exactly two selectable strategies');
select ok(exists(select 1 from platform_store.house_agent_host_approvals
  where host_installation_id = 'hosted-dev'
    and house_agent_revision_digest = 'blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81'
    and not available_for_new_assignments), 'retained obsolete approval is preserved but not selectable');
select ok(exists(select 1 from platform_store.house_agent_host_approvals
  where host_installation_id = 'hosted-dev'
    and house_agent_revision_digest = 'blake3:b624622cf68798968daa9548d32bddb3404298931fd532a8fd87266cc0ecf415'
    and not available_for_new_assignments), 'the retained r12 Planner approval is not selected by the r13 successor');
select * from finish();
rollback;
