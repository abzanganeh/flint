//! Global hotkey registration for Flint's Ctrl+Option/Alt chord system.
//!
//! Hotkey contract (§FR-5.11):
//!   Ctrl+Alt          — manual trigger (React handles tap/hold/double-tap timing)
//!   Ctrl+Alt+Shift    — panic hide/reveal overlay
//!
//! Hold 2s = Answer Now and double-tap = cancel are handled in the React layer
//! via event timing on `hotkey_trigger`, not as separate OS shortcuts.

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tracing::{info, warn};

use crate::events::{emit_hotkey_trigger, HotkeyTriggerPayload};

// tauri-plugin-global-shortcut requires at least one non-modifier key.
// Space is used as a neutral key that is unlikely to conflict with normal
// typing, and the chord is short enough to press with one hand.
const SHORTCUT_TRIGGER: &str = "Control+Alt+Space";
const SHORTCUT_PANIC: &str = "Control+Alt+Shift+Space";

/// Register all Flint global shortcuts.
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
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
    emit_hotkey_trigger(
        app,
        HotkeyTriggerPayload {
            action: "tap".to_string(),
        },
    );
}

fn toggle_overlay<R: Runtime>(app: &AppHandle<R>) {
    use crate::events::{emit_overlay_visibility, OverlayVisibilityPayload};

    let Some(state) = app.try_state::<crate::state::AppState>() else {
        return;
    };
    let mut hidden = state
        .overlay_panic_hidden
        .lock()
        .expect("overlay_panic_hidden lock poisoned");
    *hidden = !*hidden;
    emit_overlay_visibility(
        app,
        OverlayVisibilityPayload {
            hidden: *hidden,
        },
    );
    info!(hidden = *hidden, event = "overlay_panic_toggled");
}
