# Hosted Supabase runbook (manual gate)

Cloud Supabase project creation requires a human in the [Supabase dashboard](https://supabase.com/dashboard).
This runbook covers migration verification, production configuration, and go-live checks.

## Local migration verification (repeat before every release)

From the repo root with Docker running:

```bash
npx supabase stop --no-backup
npx supabase start   # applies all supabase/migrations/*.sql in timestamp order
```

Expected migration order (verified 2026-07-02):

| Order | File | Purpose |
| --- | --- | --- |
| 1 | `20260526232200_initial_schema.sql` | Core tables + RLS policies |
| 2 | `20260618120000_global_question_bank.sql` | pgvector question bank + read policies |
| 3 | `20260701120000_auth_users_sync.sql` | `auth.users` → `public.users` trigger + backfill |

All three must apply without error on a fresh `supabase start`.

### RLS sanity check

```bash
psql "$DATABASE_URL" -c "
SELECT c.relname AS table_name, c.relrowsecurity AS rls_enabled
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind = 'r'
ORDER BY c.relname;
"
```

Every `public` table must show `rls_enabled = t`:

- `credentials`, `global_question_bank`, `profiles`, `responses`
- `session_insights`, `sessions`, `templates`, `transcripts`, `users`

### Auth sync trigger check

```bash
psql "$DATABASE_URL" -c "
SELECT tgname FROM pg_trigger t
JOIN pg_class c ON c.oid = t.tgrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'auth' AND c.relname = 'users' AND NOT t.tgisinternal;
"
```

Expect: `on_auth_user_created`, `on_auth_user_updated`.

Local DB URL: `postgresql://postgres:postgres@127.0.0.1:54322/postgres` (`npx supabase status`).

---

## Create hosted project (manual — dashboard)

1. **Supabase dashboard → New project**
   - Choose region close to your users.
   - Save the database password securely.

2. **Link CLI to the remote project**

   ```bash
   npx supabase login
   npx supabase link --project-ref <PROJECT_REF>
   ```

3. **Push migrations**

   ```bash
   npx supabase db push
   ```

   Confirm the same three migrations apply. Do **not** hand-edit schema in the SQL editor.

4. **Copy API credentials**
   - Project Settings → API → **Project URL** and **anon public** key.
   - Never commit the service role key to the desktop app.

---

## Production auth URLs

Flint reads Supabase at runtime via environment variables (not committed secrets):

| Variable | Production value |
| --- | --- |
| `FLINT_SUPABASE_URL` | `https://<PROJECT_REF>.supabase.co` |
| `FLINT_SUPABASE_ANON_KEY` | Anon key from dashboard |

Set these in your release/build pipeline or OS keychain bootstrap — see `src-tauri/src/supabase/config.rs`.

### Supabase Auth redirect allow-list

In **Authentication → URL configuration**:

| Setting | Value |
| --- | --- |
| Site URL | Your production landing or app origin (e.g. `https://flint.app`) |
| Redirect URLs | `flint://auth/callback` (required for desktop OAuth) |
| | Production web callback if used (e.g. `https://flint.app/oauth-callback.html`) |

Local dev mirrors this in `supabase/config.toml` → `[auth].additional_redirect_urls`.

### OAuth providers (optional)

Enable Google (or others) under **Authentication → Providers**. Set secrets via Supabase dashboard env vars — e.g. `SUPABASE_AUTH_EXTERNAL_GOOGLE_SECRET` referenced in `supabase/config.toml`.

---

## Go-live checklist (manual gate)

Before marking hosted Supabase closed in release docs:

- [ ] Cloud project created; `supabase db push` succeeded on empty project
- [ ] All 9 `public` tables have RLS enabled (query above on hosted DB)
- [ ] Sign-up creates row in both `auth.users` and `public.users` (trigger test)
- [ ] `FLINT_SUPABASE_URL` + `FLINT_SUPABASE_ANON_KEY` set in production build
- [ ] `flint://auth/callback` in hosted redirect allow-list; Google OAuth tested end-to-end
- [ ] Service role key **not** shipped in the desktop binary

Until then, leave **hosted Supabase** in `manual_gate_backlog`.
