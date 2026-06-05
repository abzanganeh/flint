pub mod audio;
mod auth_session;
mod commands;
pub mod confidence;
pub mod cost;
pub mod digest;
mod dto;
mod events;
pub mod flags;
pub mod gdpr;
mod health;
mod hotkeys;
pub mod interfaces;
mod keychain;
pub mod llm;
pub mod orchestrator;
pub mod rag;
pub mod session;
mod state;
mod stealth;
mod supabase;
pub mod transcription;

use crate::events::{emit_session_state_change, SessionStateChangePayload};
use tauri::Manager;

/// Initialise structured logging.
///
/// Release builds default to INFO and drop DEBUG entirely (content-bearing
/// fields are also gated behind `#[cfg(debug_assertions)]` at call sites,
/// belt-and-braces). Operators can override via `FLINT_LOG=…` using the
/// standard `tracing_subscriber::EnvFilter` syntax. Idempotent — safe to
/// call from tests via the public re-export.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    #[cfg(debug_assertions)]
    let default_filter = "info,flint=debug";
    #[cfg(not(debug_assertions))]
    let default_filter = "info";

    let filter =
        EnvFilter::try_from_env("FLINT_LOG").unwrap_or_else(|_| EnvFilter::new(default_filter));

    // `try_init` swallows the AlreadyInit error so a second call (from a
    // unit test, for example) is a no-op rather than a panic.
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_ansi(cfg!(debug_assertions))
        .try_init();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            health::hardware::assess_hardware();
            let app_state = state::AppState::new(app)?;
            let restored = tauri::async_runtime::block_on(app_state.restore_auth_from_keychain());
            if restored {
                emit_session_state_change(
                    app.handle(),
                    SessionStateChangePayload {
                        state: "IDLE".to_string(),
                    },
                );
            }
            app.manage(app_state);

            // Phase 7.6 — kick off a non-blocking flag refresh in the
            // background. The compiled-in defaults are already loaded so
            // the UI works immediately; this just upgrades to the latest
            // remote values when Supabase responds.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = app_handle.try_state::<state::AppState>() {
                    if let Some(source) = flags::supabase_source_from_plugins(&state.plugins) {
                        if let Err(e) = state.feature_flags.refresh_from(&source).await {
                            tracing::warn!(error = %e, "initial feature flag refresh failed");
                        }
                    }
                }
            });

            hotkeys::register_hotkeys(app.handle());
            stealth::apply_capture_exclusion(app.handle());
            #[cfg(debug_assertions)]
            stealth::configure_dev_window(app.handle());
            #[cfg(not(debug_assertions))]
            stealth::place_on_non_primary_monitor(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Auth (Phase 1)
            commands::get_legal_consent_accepted,
            commands::set_legal_consent_accepted,
            commands::signup,
            commands::set_session_state,
            commands::login,
            commands::logout,
            commands::get_current_user,
            commands::get_hardware_profile,
            commands::run_health_check,
            // Session design (Phase 2)
            commands::create_session,
            commands::ingest_context,
            commands::confirm_digest,
            commands::get_digest,
            commands::get_session_context,
            commands::get_session_snapshot,
            commands::get_rehearsal_completed,
            commands::run_rehearsal_turn,
            commands::complete_rehearsal,
            // Live session (Phase 3+)
            commands::start_session,
            commands::stop_session,
            commands::trigger_response,
            commands::cancel_inference,
            commands::panic_hide_overlay,
            commands::switch_provider,
            // Phase 6 — crash recovery + post-session
            commands::check_crash_recovery,
            commands::resume_crashed_session,
            commands::discard_crashed_session,
            commands::discard_all_crashed_sessions,
            commands::generate_session_summary,
            commands::list_sessions,
            commands::promote_session,
            commands::demote_session,
            commands::delete_session,
            // Phase 7.4 — cost cap enforcement
            commands::get_cost_status,
            commands::set_cost_cap,
            commands::lift_cost_suspension,
            commands::reset_cost_tracker,
            // Phase 7.5 — GDPR right-to-deletion + right-to-export
            commands::delete_account,
            commands::export_user_data,
            // Phase 7.6 — feature flags
            commands::is_feature_enabled,
            commands::refresh_feature_flags,
            commands::get_feature_flags_snapshot,
            // Phase 7.7 — provider API key management
            commands::save_provider_key,
            commands::is_provider_key_present,
            commands::clear_provider_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
