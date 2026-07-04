-- Sync auth.users -> public.users on signup, login metadata updates, and backfill.

create or replace function public.sync_auth_user_to_public_users()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
declare
    user_email text;
begin
    user_email := coalesce(
        nullif(trim(new.email), ''),
        nullif(trim(new.raw_user_meta_data ->> 'email'), ''),
        'unknown+' || new.id::text || '@flint.local'
    );

    insert into public.users (id, email, created_at)
    values (
        new.id,
        user_email,
        coalesce(new.created_at, now())
    )
    on conflict (id) do update
        set email = excluded.email;

    return new;
end;
$$;

revoke all on function public.sync_auth_user_to_public_users() from public;
grant execute on function public.sync_auth_user_to_public_users() to supabase_auth_admin;

create trigger on_auth_user_created
    after insert on auth.users
    for each row
    execute function public.sync_auth_user_to_public_users();

create trigger on_auth_user_updated
    after update of email, raw_user_meta_data on auth.users
    for each row
    execute function public.sync_auth_user_to_public_users();

-- Backfill auth users that predate this migration (or missed the trigger).
insert into public.users (id, email, created_at)
select
    au.id,
    coalesce(
        nullif(trim(au.email), ''),
        nullif(trim(au.raw_user_meta_data ->> 'email'), ''),
        'unknown+' || au.id::text || '@flint.local'
    ) as email,
    coalesce(au.created_at, now()) as created_at
from auth.users as au
on conflict (id) do update
    set email = excluded.email;
