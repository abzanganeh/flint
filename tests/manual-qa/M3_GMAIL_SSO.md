# M3 Gmail SSO — Manual Test Guide (Block E)

Branch: `feature/m3-mock-interview`  
Platform: Linux (Wayland) first; macOS/Windows use same `flint://auth/callback` flow  
Redirect URI: `flint://auth/callback`

## One-time Supabase + Google setup

1. **Supabase Dashboard** → Authentication → Providers → **Google** → Enable.
2. Add **Redirect URL**: `flint://auth/callback`  
   (Authentication → URL Configuration → Redirect URLs)
3. **Google Cloud Console** → OAuth 2.0 Client (Web application):
   - Authorized redirect URI: your Supabase callback, e.g.  
     `https://<project-ref>.supabase.co/auth/v1/callback`
4. Copy Google Client ID + Secret into Supabase Google provider settings.
5. Ensure Flint `.env` has:
   - `FLINT_SUPABASE_URL`
   - `FLINT_SUPABASE_ANON_KEY`

## Linux deep-link registration (dev)

OAuth callback requires the OS to forward `flint://auth/callback?...` to Flint:

```bash
cd /home/alireza/Desktop/projects/Flint
npm run deeplink:register   # once per machine / after rebuild
npm run tauri dev
```

Release builds register the scheme automatically (`deep_link.register_all`).

## Test E1 — New user Google sign-up

Prerequisites: legal consent not yet accepted OR sign out first (Settings → Account → Sign out).

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Launch Flint → accept legal notice | Auth screen shows **Continue with Google** |
| 2 | Click **Continue with Google** | System browser opens Google account picker |
| 3 | Choose Google account / complete consent | Browser redirects; Flint window comes to foreground |
| 4 | Wait ≤ 5 s | Onboarding completes → Health Check or home (no error toast) |
| 5 | Settings → confirm logged in | Email matches Google account |

**Pass:** lands past onboarding with valid session; `get_current_user` returns Gmail address.

## Test E2 — Returning user Google login

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Settings → Account → **Sign out** | Returned to auth / onboarding |
| 2 | **Continue with Google** (same account) | Browser flow completes |
| 3 | Flint resumes | Prior local sessions still in Session List (SQLite not wiped) |

## Test E3 — Cancel / deny

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Start Google OAuth | Browser opens; UI shows **Cancel Google sign-in** |
| 2 | Click **Cancel Google sign-in** (in-app) OR deny in browser | Error shown; can retry email or Google |
| 3 | Browser deny via Supabase→Vite redirect | `index.html` forwards `?error=` to `flint://auth/callback`; Flint shows error |

**E3 pass (2026-06-17):** `cancel_google_oauth` clears PKCE + emits `auth_oauth_error`; Vite dev `index.html` bridges OAuth errors to deep link.

## Test E4 — Email/password still works

| Step | Action | Pass criteria |
|------|--------|---------------|
| 1 | Sign out | Auth screen |
| 2 | Email + password login (existing Supabase user) | Works as before |

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Browser completes but Flint stuck on "Waiting for Google…" | Deep link not registered | `npm run deeplink:register`; retry |
| `OAuth session expired` | PKCE verifier lost (second instance race) | Single Flint instance; retry OAuth |
| `redirect_uri_mismatch` | Supabase redirect list | Add `flint://auth/callback` exactly |
| Browser deny lands on Vite localhost with `?error=` | Supabase `site_url` is Vite in dev | `index.html` redirects to `flint://auth/callback` + error params |
| `Flint could not reach the auth service` | Wrong Supabase URL/key | Check `.env` |

## Verify tokens (optional)

```bash
# After successful login — keychain should hold auth entries (Linux secret-service)
secret-tool lookup service flint account auth_token_access
```

Do **not** log or paste token values.

## Loop message when Block E passes

```
/flint-loop resume — m3-gmail-sso Block E pass (E1–E4 Linux)
```

**Status:** PASS 2026-06-17 (Linux).
