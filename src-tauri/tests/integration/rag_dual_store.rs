//! Phase 9 dual-store integration gate — session isolation, store separation,
//! slot allocation, and cleanup.
//!
//! Reference: ROADMAP Phase 9 review gate, design doc §16.5.

use flint_lib::interfaces::vector::{Chunk, VectorInterface};
use flint_lib::rag::embedder::Embedder;
use flint_lib::rag::retriever::retrieve_for_prompt;
use flint_lib::rag::store::SqliteVecStore;
use uuid::Uuid;

fn embedder() -> Option<Embedder> {
    Embedder::new_if_cached()
}

fn make_chunk(text: &str, session_id: Uuid, embedder: &Embedder) -> Chunk {
    let embedding = embedder.embed_one(text).expect("embed failed");
    Chunk {
        id: Uuid::new_v4(),
        text: text.to_string(),
        embedding,
        session_id,
    }
}

#[tokio::test]
async fn test_session_a_qa_does_not_leak_into_session_b_context() {
    let Some(emb) = embedder() else {
        eprintln!("SKIP rag_dual_store: fastembed model not cached");
        return;
    };

    let store = SqliteVecStore::new(":memory:").expect("store should open");
    let session_a = Uuid::new_v4();
    let session_b = Uuid::new_v4();

    store
        .ingest_qa(
            session_a,
            vec![make_chunk(
                "Q: Describe your IAM experience\nA: Led enterprise SSO rollouts",
                session_a,
                &emb,
            )],
        )
        .await
        .unwrap();

    store
        .ingest_context(
            session_b,
            vec![make_chunk(
                "Senior backend engineer job description",
                session_b,
                &emb,
            )],
        )
        .await
        .unwrap();

    let query = emb.embed_one("IAM experience").unwrap();
    let session_b_chunks = retrieve_for_prompt(&store, session_b, &query)
        .await
        .unwrap();

    assert!(
        session_b_chunks.qa.is_empty(),
        "session B must not retrieve Q&A chunks from session A"
    );
    assert!(
        session_b_chunks
            .context
            .iter()
            .all(|c| !c.chunk.text.contains("IAM experience")),
        "session B context must not include session A Q&A text"
    );
}

#[tokio::test]
async fn test_user_qa_pair_in_context_store_is_retrievable() {
    let Some(emb) = embedder() else {
        eprintln!("SKIP rag_dual_store: fastembed model not cached");
        return;
    };

    let store = SqliteVecStore::new(":memory:").expect("store should open");
    let session = Uuid::new_v4();
    let pair = "Q: What is your greatest strength?\nA: Designing resilient identity platforms";

    store
        .ingest_context(session, vec![make_chunk(pair, session, &emb)])
        .await
        .unwrap();

    let query = emb.embed_one("greatest strength").unwrap();
    let chunks = retrieve_for_prompt(&store, session, &query).await.unwrap();

    assert!(
        chunks
            .context
            .iter()
            .any(|c| c.chunk.text.contains("greatest strength")),
        "user-provided Q&A pair must surface in context slots"
    );
    assert!(
        chunks.context.iter().any(|c| c.score >= 0.80),
        "retrieved user Q&A chunk should clear similarity threshold"
    );
}
