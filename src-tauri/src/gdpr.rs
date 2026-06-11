//! Phase 7.5 — GDPR right-to-deletion + right-to-export.
//!
//! Coordinates the four cleanup paths that must all run when a user clicks
//! Settings → Delete Account:
//!
//! 1. **Supabase** — call [`AuthInterface::delete_account`] so the auth row
//!    and all RLS-scoped session rows are removed server-side.
//! 2. **OS keychain** — purge auth tokens + consent flags via
//!    [`keychain::clear_account_secrets`]. BYOK API keys are preserved.
//! 3. **Local vector store** — drop each session's `vec_chunks_{hex}` table
//!    so embedding data is gone alongside the relational rows.
//! 4. **Local SQLite** — truncate every user-data table in one transaction
//!    via [`SessionPersistence::clear_all_user_data`].
//!
//! Each step is best-effort: a failure on any one step still permits the
//! others to run. The returned [`DeleteAccountReport`] describes which
//! steps succeeded so the UI can prompt the user to retry server-side
//! deletion (or contact support) without losing the local cleanup.
//!
//! ## Why not transactional?
//!
//! The four backends live in different processes and trust boundaries.
//! Treating them transactionally would require a distributed protocol Flint
//! does not have. The pragmatic alternative is to always clear local state
//! eagerly — that satisfies the strict GDPR requirement that local copies
//! be removed immediately — and to surface server-side failures to the
//! user.

use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use tracing::{info, warn};

use crate::interfaces::auth::{AuthInterface, AuthToken};
use crate::interfaces::vector::VectorInterface;
use crate::session::persistence::{SessionExport, SessionPersistence};

/// Outcome of each cleanup step. Returned from
/// [`delete_account`] so the UI can render a precise summary.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteAccountReport {
    pub supabase_deleted: bool,
    pub supabase_error: Option<String>,
    pub keychain_cleared: bool,
    pub keychain_error: Option<String>,
    pub vector_store_cleared: bool,
    pub vector_store_error: Option<String>,
    pub sqlite_cleared: bool,
    pub sqlite_error: Option<String>,
    pub sessions_cleared: usize,
}

impl DeleteAccountReport {
    /// True when every step succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.supabase_deleted
            && self.keychain_cleared
            && self.vector_store_cleared
            && self.sqlite_cleared
    }
}

/// Abstraction for the keychain cleanup step. The production wiring passes
/// [`crate::keychain::clear_account_secrets`]; tests can pass a stub so
/// they don't race against the user's real OS keychain.
pub type KeychainPurge = Box<dyn FnOnce() -> Result<()> + Send>;

/// Run all four cleanup steps. Returns a per-step report; never bubbles a
/// failure as an `Err` because the caller wants to render partial success.
pub async fn delete_account(
    auth: Arc<dyn AuthInterface>,
    token: AuthToken,
    persistence: Arc<SessionPersistence>,
    vector_store: Arc<dyn VectorInterface>,
    purge_keychain: KeychainPurge,
) -> DeleteAccountReport {
    // 1. Supabase — server-side row + auth user.
    let (supabase_deleted, supabase_error) = match auth.delete_account(&token).await {
        Ok(()) => (true, None),
        Err(e) => {
            warn!(error = %e, "Supabase account deletion failed");
            (false, Some(redact_for_ui(&e.to_string())))
        }
    };

    // 2. Local vector store — iterate every known session before SQLite is
    // truncated, otherwise we lose the list of vector tables to drop.
    let session_ids = match persistence.list_all_session_ids() {
        Ok(ids) => ids,
        Err(e) => {
            warn!(error = %e, "failed to enumerate sessions for vector wipe");
            Vec::new()
        }
    };

    let mut vector_error: Option<String> = None;
    for id in &session_ids {
        if let Err(e) = vector_store.delete_session(*id).await {
            warn!(session_id = %id, error = %e, "vector store delete failed");
            if vector_error.is_none() {
                vector_error = Some(redact_for_ui(&e.to_string()));
            }
        }
    }
    let vector_store_cleared = vector_error.is_none();

    // 3. Local SQLite — clear every user-data row atomically.
    let (sqlite_cleared, sqlite_error) = match persistence.clear_all_user_data() {
        Ok(()) => (true, None),
        Err(e) => {
            warn!(error = %e, "local SQLite wipe failed");
            (false, Some(redact_for_ui(&e.to_string())))
        }
    };

    // 4. Keychain — purge auth tokens, consent flags, and known API keys.
    let (keychain_cleared, keychain_error) = match purge_keychain() {
        Ok(()) => (true, None),
        Err(e) => {
            warn!(error = %e, "keychain wipe failed");
            (false, Some(redact_for_ui(&e.to_string())))
        }
    };

    info!(
        supabase = supabase_deleted,
        keychain = keychain_cleared,
        vector = vector_store_cleared,
        sqlite = sqlite_cleared,
        sessions = session_ids.len(),
        "delete_account complete"
    );

    DeleteAccountReport {
        supabase_deleted,
        supabase_error,
        keychain_cleared,
        keychain_error,
        vector_store_cleared,
        vector_store_error: vector_error,
        sqlite_cleared,
        sqlite_error,
        sessions_cleared: session_ids.len(),
    }
}

/// Wrapper for the data shape returned by [`export_user_data`]. Includes
/// schema versioning so future Flint versions can refuse to import payloads
/// from incompatible builds.
#[derive(Debug, Clone, Serialize)]
pub struct UserDataExport {
    pub schema_version: u32,
    pub exported_at: i64,
    pub sessions: Vec<SessionExport>,
}

const EXPORT_SCHEMA_VERSION: u32 = 1;

/// Dump every locally-stored session into a structured export. The caller
/// is responsible for serialising and persisting the result (e.g. writing
/// to disk from the frontend).
pub fn export_user_data(persistence: &SessionPersistence) -> Result<UserDataExport> {
    let sessions = persistence.export_all_data()?;
    Ok(UserDataExport {
        schema_version: EXPORT_SCHEMA_VERSION,
        exported_at: chrono::Utc::now().timestamp(),
        sessions,
    })
}

/// Strip anything that looks like an access/refresh token from an error
/// string before it is shown to the user. The keyring + reqwest crates do
/// not include secrets in their `Display` impls today, but defence-in-depth
/// keeps this from regressing.
fn redact_for_ui(raw: &str) -> String {
    // Quick heuristic: collapse anything that looks like a long token.
    let mut out = String::with_capacity(raw.len());
    let mut buf = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            buf.push(ch);
        } else {
            if buf.len() >= 32 {
                out.push_str("[redacted]");
            } else {
                out.push_str(&buf);
            }
            buf.clear();
            out.push(ch);
        }
    }
    if buf.len() >= 32 {
        out.push_str("[redacted]");
    } else {
        out.push_str(&buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::auth::{Plan, User};
    use crate::interfaces::vector::Chunk;
    use async_trait::async_trait;
    use secrecy::SecretString;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use uuid::Uuid;

    // ── Fixtures ─────────────────────────────────────────────────────────

    struct ScriptedAuth {
        delete_fails: AtomicBool,
        delete_calls: AtomicUsize,
    }

    impl ScriptedAuth {
        fn new(delete_fails: bool) -> Self {
            Self {
                delete_fails: AtomicBool::new(delete_fails),
                delete_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl AuthInterface for ScriptedAuth {
        async fn signup(&self, _email: &str, _password: &str) -> Result<User> {
            unreachable!()
        }
        async fn login(&self, _email: &str, _password: &str) -> Result<AuthToken> {
            unreachable!()
        }
        async fn logout(&self, _token: &AuthToken) -> Result<()> {
            Ok(())
        }
        async fn refresh(&self, _refresh_token: &SecretString) -> Result<AuthToken> {
            unreachable!()
        }
        async fn get_current_user(&self, _token: &AuthToken) -> Result<User> {
            Ok(User {
                id: Uuid::new_v4(),
                email: "test@example.com".into(),
                plan: Plan::Free,
            })
        }
        async fn delete_account(&self, _token: &AuthToken) -> Result<()> {
            self.delete_calls.fetch_add(1, Ordering::SeqCst);
            if self.delete_fails.load(Ordering::SeqCst) {
                anyhow::bail!("Supabase 503");
            }
            Ok(())
        }
    }

    fn test_token() -> AuthToken {
        AuthToken {
            access_token: SecretString::new("access".into()),
            refresh_token: SecretString::new("refresh".into()),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        }
    }

    struct StubVectorStore {
        delete_fails: AtomicBool,
        delete_calls: AtomicUsize,
    }

    impl StubVectorStore {
        fn new(delete_fails: bool) -> Self {
            Self {
                delete_fails: AtomicBool::new(delete_fails),
                delete_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl VectorInterface for StubVectorStore {
        async fn ingest(&self, _session_id: Uuid, _chunks: Vec<Chunk>) -> Result<()> {
            Ok(())
        }
        async fn query(
            &self,
            _session_id: Uuid,
            _embedding: &[f32],
            _top_k: usize,
        ) -> Result<Vec<crate::interfaces::vector::ScoredChunk>> {
            Ok(Vec::new())
        }
        async fn delete_session(&self, _session_id: Uuid) -> Result<()> {
            self.delete_calls.fetch_add(1, Ordering::SeqCst);
            if self.delete_fails.load(Ordering::SeqCst) {
                anyhow::bail!("vector store offline");
            }
            Ok(())
        }
        fn chunk_count(&self, _session_id: Uuid) -> usize {
            0
        }
    }

    fn seeded_persistence(n_sessions: usize) -> Arc<SessionPersistence> {
        let p = Arc::new(SessionPersistence::new(":memory:").expect("in-memory persistence"));
        for i in 0..n_sessions {
            let id = Uuid::new_v4();
            p.create_session_row(id, &format!("Session {i}"), "interview", "swe")
                .expect("create session row");
        }
        p
    }

    // ── Tests ────────────────────────────────────────────────────────────

    /// Returns `(purge_fn, hit_flag)`. `hit_flag` flips to `true` when the
    /// closure is invoked so tests can assert keychain cleanup ran without
    /// touching the real OS keychain.
    fn stub_keychain_purge() -> (KeychainPurge, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = Arc::clone(&flag);
        let purge: KeychainPurge = Box::new(move || {
            flag_clone.store(true, Ordering::SeqCst);
            Ok(())
        });
        (purge, flag)
    }

    fn failing_keychain_purge() -> KeychainPurge {
        Box::new(|| anyhow::bail!("keychain unavailable"))
    }

    #[tokio::test]
    async fn happy_path_wipes_local_state_and_calls_supabase() {
        let persistence = seeded_persistence(3);
        let vector: Arc<StubVectorStore> = Arc::new(StubVectorStore::new(false));
        let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(false));
        let (purge, purge_hit) = stub_keychain_purge();

        let report = delete_account(
            auth.clone(),
            test_token(),
            persistence.clone(),
            vector.clone(),
            purge,
        )
        .await;

        assert!(report.all_succeeded(), "report: {report:?}");
        assert_eq!(report.sessions_cleared, 3);
        assert_eq!(auth.delete_calls.load(Ordering::SeqCst), 1);
        assert_eq!(vector.delete_calls.load(Ordering::SeqCst), 3);
        assert!(purge_hit.load(Ordering::SeqCst), "keychain purge invoked");
        assert_eq!(
            persistence.list_all_session_ids().unwrap().len(),
            0,
            "all sessions purged"
        );
    }

    #[tokio::test]
    async fn supabase_failure_still_wipes_local_state() {
        let persistence = seeded_persistence(2);
        let vector: Arc<StubVectorStore> = Arc::new(StubVectorStore::new(false));
        let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(true));
        let (purge, purge_hit) = stub_keychain_purge();

        let report = delete_account(
            auth.clone(),
            test_token(),
            persistence.clone(),
            vector.clone(),
            purge,
        )
        .await;

        assert!(!report.supabase_deleted);
        assert!(report.supabase_error.is_some());
        assert!(report.sqlite_cleared, "local cleanup must still run");
        assert!(purge_hit.load(Ordering::SeqCst), "keychain still purged");
        assert_eq!(persistence.list_all_session_ids().unwrap().len(), 0);
        assert_eq!(vector.delete_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn vector_store_failure_does_not_block_other_steps() {
        let persistence = seeded_persistence(1);
        let vector: Arc<StubVectorStore> = Arc::new(StubVectorStore::new(true));
        let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(false));
        let (purge, _) = stub_keychain_purge();

        let report = delete_account(auth, test_token(), persistence.clone(), vector, purge).await;

        assert!(report.supabase_deleted);
        assert!(!report.vector_store_cleared);
        assert!(report.vector_store_error.is_some());
        // SQLite is still cleared because the rows were already counted.
        assert!(report.sqlite_cleared);
        assert_eq!(persistence.list_all_session_ids().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn keychain_failure_does_not_block_other_steps() {
        let persistence = seeded_persistence(1);
        let vector: Arc<StubVectorStore> = Arc::new(StubVectorStore::new(false));
        let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(false));

        let report = delete_account(
            auth,
            test_token(),
            persistence.clone(),
            vector,
            failing_keychain_purge(),
        )
        .await;

        assert!(report.supabase_deleted);
        assert!(report.sqlite_cleared);
        assert!(!report.keychain_cleared);
        assert!(report.keychain_error.is_some());
        assert_eq!(persistence.list_all_session_ids().unwrap().len(), 0);
    }

    #[test]
    fn export_user_data_round_trips_minimum_fields() {
        let persistence = SessionPersistence::new(":memory:").expect("in-memory persistence");
        let id = Uuid::new_v4();
        persistence
            .create_session_row(id, "Export Test", "interview", "swe")
            .unwrap();

        let export = export_user_data(&persistence).expect("export ok");
        assert_eq!(export.schema_version, EXPORT_SCHEMA_VERSION);
        assert_eq!(export.sessions.len(), 1);
        assert_eq!(export.sessions[0].id, id);
        assert_eq!(export.sessions[0].name, "Export Test");
    }

    #[test]
    fn redact_for_ui_strips_long_tokens() {
        let raw = "Failed to call /user with bearer abcdefghijklmnopqrstuvwxyz0123456789";
        let out = redact_for_ui(raw);
        assert!(out.contains("[redacted]"), "expected redaction in {out:?}");
        assert!(
            !out.contains("abcdefghijklmnopqrstuvwxyz"),
            "raw token leaked: {out:?}"
        );
    }

    #[test]
    fn redact_for_ui_preserves_short_strings() {
        let raw = "Supabase 503";
        let out = redact_for_ui(raw);
        assert_eq!(out, raw);
    }
}
