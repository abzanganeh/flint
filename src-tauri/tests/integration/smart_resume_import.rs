//! Smart Resume handoff import command — HTTP error mapping.

use std::sync::OnceLock;

use flint_lib::smart_resume;
use tokio::sync::Mutex;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn import_from_smart_resume_maps_success_payload() {
    let _guard = env_lock().lock().await;
    let server = MockServer::start().await;
    std::env::set_var("FLINT_SMART_RESUME_URL", server.uri());

    Mock::given(method("POST"))
        .and(path("/api/flint/context"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "session_name": "Acme — Interview",
            "session_type": "interview",
            "domain": "software engineering",
            "jd_text": "Build distributed systems at Acme.",
            "resume_summary": "Senior engineer with Rust experience.",
            "smart_resume_session_id": "sr-session-1",
            "export_version": 1
        })))
        .mount(&server)
        .await;

    let result = smart_resume::redeem_handoff_token("550e8400-e29b-41d4-a716-446655440000")
        .await
        .expect("import succeeds");

    assert_eq!(result.session_name, "Acme — Interview");
    assert_eq!(result.jd_text, "Build distributed systems at Acme.");
    assert_eq!(result.smart_resume_session_id, "sr-session-1");

    std::env::remove_var("FLINT_SMART_RESUME_URL");
}

#[tokio::test]
async fn fetch_interview_questions_maps_success_payload() {
    let _guard = env_lock().lock().await;
    let server = MockServer::start().await;
    std::env::set_var("FLINT_SMART_RESUME_URL", server.uri());

    Mock::given(method("GET"))
        .and(path("/api/interview-questions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "domain": "software_engineering",
            "questions": [
                {
                    "id": "uni-1",
                    "text": "Tell me about yourself.",
                    "domain": "universal",
                    "category": "introduction",
                    "canonical_answer": null
                }
            ],
            "total": 1
        })))
        .mount(&server)
        .await;

    let result = smart_resume::fetch_interview_questions(
        "software engineering",
        "Acme",
        "Staff Engineer",
        30,
    )
    .await
    .expect("fetch succeeds");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].text, "Tell me about yourself.");

    std::env::remove_var("FLINT_SMART_RESUME_URL");
}

#[tokio::test]
async fn import_from_smart_resume_maps_expired_token() {
    let _guard = env_lock().lock().await;
    let server = MockServer::start().await;
    std::env::set_var("FLINT_SMART_RESUME_URL", server.uri());

    Mock::given(method("POST"))
        .and(path("/api/flint/context"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "detail": "Link expired or already used"
        })))
        .mount(&server)
        .await;

    let err = smart_resume::redeem_handoff_token("expired-token-123456789012345678901234")
        .await
        .expect_err("expired token");

    assert!(err.contains("expired"));

    std::env::remove_var("FLINT_SMART_RESUME_URL");
}
