pub mod audio;
mod auth_session;
mod commands;
pub mod confidence;
pub mod digest;
mod dto;
mod events;
mod health;
mod hotkeys;
pub mod interfaces;
mod keychain;
pub mod llm;
pub mod orchestrator;
pub mod rag;
pub mod session;
mod state;
mod supabase;
pub mod transcription;

use crate::events::{emit_session_state_change, SessionStateChangePayload};
use tauri::Manager;
use tauri_plugin_global_shortcut;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            hotkeys::register_hotkeys(app.handle());
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
