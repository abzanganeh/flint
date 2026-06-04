//! Global hotkey registration for Flint's Ctrl+Option/Alt chord system.
//!
//! Hotkey contract (§FR-5.11):
//!   Ctrl+Alt          — manual trigger (fire orchestrator turn)
//!   Ctrl+Alt+Shift    — panic hide/reveal overlay
//!
//! "Hold 2s = Answer Now" and "double-tap = cancel" are handled in the React
//! layer via event timing on top of the tap event, not registered as separate
//! OS shortcuts (OS global shortcuts do not expose hold/double-tap semantics).

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tracing::{info, warn};

use crate::events::{emit_session_state_change, SessionStateChangePayload};

const SHORTCUT_TRIGGER: &str = "Ctrl+Alt+T";
const SHORTCUT_PANIC: &str = "Ctrl+Alt+Shift+H";

/// Register all Flint global shortcuts.
///
/// Called once from `lib.rs::run()` during app setup. Safe to call on every
/// platform; on Wayland/X11 desktop environments the OS may reject shortcuts
/// that conflict with the compositor — we log a warning and continue rather
/// than blocking startup.
pub fn register_hotkeys<R: Runtime>(app: &AppHandle<R>) {
    let app_trigger = app.clone();
    let app_panic = app.clone();

    let trigger: Shortcut = match SHORTCUT_TRIGGER.parse() {
        Ok(s) => s,
        Err(e) => {
            warn!(shortcut = SHORTCUT_TRIGGER, error = %e, "failed to parse trigger shortcut");
            return;
        }
    };

    let panic_hide: Shortcut = match SHORTCUT_PANIC.parse() {
        Ok(s) => s,
        Err(e) => {
            warn!(shortcut = SHORTCUT_PANIC, error = %e, "failed to parse panic shortcut");
            return;
        }
    };

    if let Err(e) = app
        .global_shortcut()
        .on_shortcut(trigger, move |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                info!(event = "hotkey_trigger");
                fire_trigger(&app_trigger);
            }
        })
    {
        warn!(shortcut = SHORTCUT_TRIGGER, error = %e, "failed to register trigger shortcut");
    }

    if let Err(e) = app
        .global_shortcut()
        .on_shortcut(panic_hide, move |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                info!(event = "hotkey_panic_hide");
                toggle_overlay(&app_panic);
            }
        })
    {
        warn!(shortcut = SHORTCUT_PANIC, error = %e, "failed to register panic shortcut");
    }

    info!(
        trigger = SHORTCUT_TRIGGER,
        panic = SHORTCUT_PANIC,
        event = "hotkeys_registered"
    );
}

fn fire_trigger<R: Runtime>(app: &AppHandle<R>) {
    // Emit a state-change-adjacent event so the React layer knows a manual
    // trigger fired. The orchestrator picks it up via the trigger_response
    // command — here we just make the window visible and forward the signal.
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
    // Emit a thin JSON event that the React layer listens to.
    let _ = app.emit("hotkey_trigger", ());
}

fn toggle_overlay<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(true);
        if visible {
            let _ = win.hide();
            info!(event = "overlay_hidden");
        } else {
            let _ = win.show();
            let _ = win.set_focus();
            info!(event = "overlay_revealed");
            // Re-emit IDLE so React re-renders if it missed state while hidden.
            emit_session_state_change(
                app,
                SessionStateChangePayload {
                    state: "IDLE".to_string(),
                },
            );
        }
    }
}
