//! Phase 7.5 — end-to-end GDPR delete account flow.
//!
//! Exercises [`flint_lib::gdpr::delete_account`] against a real
//! `SqliteVecStore` and `SessionPersistence` plus a scripted auth mock and a
//! stub keychain closure. Verifies that every backing store is wiped and
//! that partial failures still drive the remaining steps.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use secrecy::SecretString;
use uuid::Uuid;

use flint_lib::gdpr::{delete_account, export_user_data, DeleteAccountReport};
use flint_lib::interfaces::auth::{AuthInterface, AuthToken, Plan, User};
use flint_lib::interfaces::vector::{Chunk, VectorInterface};
use flint_lib::rag::store::SqliteVecStore;
use flint_lib::session::persistence::SessionPersistence;

// ── Test fixtures ────────────────────────────────────────────────────────────

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
    async fn signup(&self, _e: &str, _p: &str) -> anyhow::Result<User> {
        unreachable!("signup not used in GDPR tests")
    }
    async fn login(&self, _e: &str, _p: &str) -> anyhow::Result<AuthToken> {
        unreachable!("login not used in GDPR tests")
    }
    async fn logout(&self, _token: &AuthToken) -> anyhow::Result<()> {
        Ok(())
    }
    async fn refresh(&self, _refresh: &SecretString) -> anyhow::Result<AuthToken> {
        unreachable!("refresh not used in GDPR tests")
    }
    async fn get_current_user(&self, _token: &AuthToken) -> anyhow::Result<User> {
        Ok(User {
            id: Uuid::new_v4(),
            email: "user@example.com".into(),
            plan: Plan::Free,
        })
    }
    async fn delete_account(&self, _token: &AuthToken) -> anyhow::Result<()> {
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

fn stub_keychain() -> (flint_lib::gdpr::KeychainPurge, Arc<AtomicBool>) {
    let flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&flag);
    let purge: flint_lib::gdpr::KeychainPurge = Box::new(move || {
        flag_clone.store(true, Ordering::SeqCst);
        Ok(())
    });
    (purge, flag)
}

fn seeded_persistence_and_vectors(
    n_sessions: usize,
) -> (Arc<SessionPersistence>, Arc<dyn VectorInterface>, Vec<Uuid>) {
    let persistence = Arc::new(SessionPersistence::new(":memory:").expect("persistence"));
    let vector: Arc<dyn VectorInterface> =
        Arc::new(SqliteVecStore::new(":memory:").expect("vector store"));

    let mut ids = Vec::with_capacity(n_sessions);
    for i in 0..n_sessions {
        let sid = Uuid::new_v4();
        persistence
            .create_session_row(sid, &format!("Session {i}"), "interview", "swe")
            .expect("create session row");
        ids.push(sid);
    }
    (persistence, vector, ids)
}

async fn seed_vectors(vector: &Arc<dyn VectorInterface>, ids: &[Uuid]) {
    for sid in ids {
        let chunk = Chunk {
            id: Uuid::new_v4(),
            text: "synthetic chunk text".to_string(),
            embedding: vec![0.0_f32; 384],
            session_id: *sid,
        };
        vector
            .ingest(*sid, vec![chunk])
            .await
            .expect("ingest chunk");
    }
}

fn assert_full_wipe(persistence: &SessionPersistence, vector: &dyn VectorInterface, ids: &[Uuid]) {
    assert!(
        persistence.list_all_session_ids().unwrap().is_empty(),
        "sqlite rows must be gone"
    );
    for sid in ids {
        assert_eq!(
            vector.chunk_count(*sid),
            0,
            "vector chunks for {sid} must be gone"
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_account_clears_sqlite_and_vector_store() {
    let (persistence, vector, ids) = seeded_persistence_and_vectors(3);
    seed_vectors(&vector, &ids).await;
    for sid in &ids {
        assert!(vector.chunk_count(*sid) > 0);
    }

    let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(false));
    let (purge, purge_hit) = stub_keychain();

    let report: DeleteAccountReport = delete_account(
        auth.clone(),
        test_token(),
        Arc::clone(&persistence),
        Arc::clone(&vector),
        purge,
    )
    .await;

    assert!(report.all_succeeded(), "report: {report:?}");
    assert_eq!(report.sessions_cleared, 3);
    assert_eq!(auth.delete_calls.load(Ordering::SeqCst), 1);
    assert!(purge_hit.load(Ordering::SeqCst), "keychain purged");
    assert_full_wipe(&persistence, vector.as_ref(), &ids);
}

#[tokio::test]
async fn delete_account_proceeds_locally_when_supabase_fails() {
    let (persistence, vector, ids) = seeded_persistence_and_vectors(2);
    seed_vectors(&vector, &ids).await;

    let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(true));
    let (purge, purge_hit) = stub_keychain();

    let report = delete_account(
        auth.clone(),
        test_token(),
        Arc::clone(&persistence),
        Arc::clone(&vector),
        purge,
    )
    .await;

    assert!(!report.supabase_deleted);
    assert!(report.supabase_error.is_some());
    assert!(report.sqlite_cleared);
    assert!(report.vector_store_cleared);
    assert!(purge_hit.load(Ordering::SeqCst));
    assert_full_wipe(&persistence, vector.as_ref(), &ids);
}

#[tokio::test]
async fn delete_account_with_no_data_is_a_safe_noop() {
    let persistence = Arc::new(SessionPersistence::new(":memory:").unwrap());
    let vector: Arc<dyn VectorInterface> = Arc::new(SqliteVecStore::new(":memory:").unwrap());
    let auth: Arc<ScriptedAuth> = Arc::new(ScriptedAuth::new(false));
    let (purge, _) = stub_keychain();

    let report = delete_account(
        auth,
        test_token(),
        Arc::clone(&persistence),
        Arc::clone(&vector),
        purge,
    )
    .await;

    assert!(report.all_succeeded());
    assert_eq!(report.sessions_cleared, 0);
}

#[tokio::test]
async fn export_user_data_serialises_round_trip() {
    let (persistence, _vector, ids) = seeded_persistence_and_vectors(2);

    let export = export_user_data(&persistence).expect("export ok");
    assert_eq!(export.schema_version, 1);
    assert_eq!(export.sessions.len(), 2);
    for sid in &ids {
        assert!(export.sessions.iter().any(|s| s.id == *sid));
    }

    let json = serde_json::to_string(&export).expect("serialise");
    assert!(json.contains("\"schema_version\":1"));
}
