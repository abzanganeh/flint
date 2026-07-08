//! Global hotkey registration for Flint's Ctrl+Option/Alt chord system.
//!
//! Hotkey contract (§FR-5.11):
//!   Ctrl+Alt+Space       — manual trigger (React handles tap/hold/double-tap timing)
//!   Ctrl+Alt+Shift+Space — panic hide/reveal overlay
//!
//! Linux / Wayland fallbacks (compositors often swallow Ctrl+Alt chords):
//!   Ctrl+Shift+Space     — focused + global re-ask fallback
//!   Ctrl+Super+Space     — Apple Option→Super mapping on Linux
//!   F8                   — debug-only global re-ask (Linux)
//! Unfocused Wayland global shortcuts remain an accepted P2 limitation.
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
/// Linux dev: Apple "Option" often maps to Super; register both chords.
#[cfg(target_os = "linux")]
const SHORTCUT_TRIGGER_META: &str = "Control+Super+Space";
#[cfg(target_os = "linux")]
const SHORTCUT_PANIC_META: &str = "Control+Super+Shift+Space";
/// Wayland often blocks Ctrl+Alt+Space before WebKit sees it; register a Linux fallback.
#[cfg(target_os = "linux")]
const SHORTCUT_TRIGGER_LINUX: &str = "Control+Shift+Space";
/// Wayland/WebKit often swallow Ctrl+Alt chords; F8 is a reliable dev fallback.
#[cfg(all(debug_assertions, target_os = "linux"))]
const SHORTCUT_TRIGGER_DEV: &str = "F8";

/// Register all Flint global shortcuts.
pub fn register_hotkeys<R: Runtime>(app: &AppHandle<R>) {
    let app_trigger = app.clone();
    let app_panic = app.clone();

    let trigger: Shortcut = match SHORTCUT_TRIGGER.parse() {
        Ok(s) => s,
        Err(e) => {
            warn!(shortcut = SHORTCUT_TRIGGER, error = %e, event = "hotkey_parse_failed");
            return;
        }
    };

    let panic_hide: Shortcut = match SHORTCUT_PANIC.parse() {
        Ok(s) => s,
        Err(e) => {
            warn!(shortcut = SHORTCUT_PANIC, error = %e, event = "hotkey_parse_failed");
            return;
        }
    };

    register_trigger_shortcut(app, trigger, SHORTCUT_TRIGGER, app_trigger);
    register_panic_shortcut(app, panic_hide, SHORTCUT_PANIC, app_panic);

    #[cfg(target_os = "linux")]
    {
        if let Ok(linux_trigger) = SHORTCUT_TRIGGER_LINUX.parse::<Shortcut>() {
            register_trigger_shortcut(app, linux_trigger, SHORTCUT_TRIGGER_LINUX, app.clone());
        }
        if let Ok(meta_trigger) = SHORTCUT_TRIGGER_META.parse::<Shortcut>() {
            register_trigger_shortcut(app, meta_trigger, SHORTCUT_TRIGGER_META, app.clone());
        }
        if let Ok(meta_panic) = SHORTCUT_PANIC_META.parse::<Shortcut>() {
            register_panic_shortcut(app, meta_panic, SHORTCUT_PANIC_META, app.clone());
        }
        #[cfg(debug_assertions)]
        if let Ok(dev_trigger) = SHORTCUT_TRIGGER_DEV.parse::<Shortcut>() {
            register_trigger_shortcut(app, dev_trigger, SHORTCUT_TRIGGER_DEV, app.clone());
        }
    }

    #[cfg(target_os = "linux")]
    {
        info!(
            trigger = SHORTCUT_TRIGGER,
            panic = SHORTCUT_PANIC,
            linux_fallback = SHORTCUT_TRIGGER_LINUX,
            event = "hotkeys_registered"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        info!(
            trigger = SHORTCUT_TRIGGER,
            panic = SHORTCUT_PANIC,
            event = "hotkeys_registered"
        );
    }
}

fn register_trigger_shortcut<R: Runtime>(
    app: &AppHandle<R>,
    shortcut: Shortcut,
    label: &'static str,
    app_trigger: AppHandle<R>,
) {
    match app
        .global_shortcut()
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                info!(event = "hotkey_trigger", shortcut = label);
                fire_trigger(&app_trigger);
            }
        }) {
        Ok(()) => info!(shortcut = label, event = "hotkey_register_ok"),
        Err(e) => warn!(shortcut = label, error = %e, event = "hotkey_register_failed"),
    }
}

fn register_panic_shortcut<R: Runtime>(
    app: &AppHandle<R>,
    shortcut: Shortcut,
    label: &'static str,
    app_panic: AppHandle<R>,
) {
    match app
        .global_shortcut()
        .on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                info!(event = "hotkey_panic_hide", shortcut = label);
                toggle_overlay(&app_panic);
            }
        }) {
        Ok(()) => info!(shortcut = label, event = "hotkey_register_ok"),
        Err(e) => warn!(shortcut = label, error = %e, event = "hotkey_register_failed"),
    }
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
    emit_overlay_visibility(app, OverlayVisibilityPayload { hidden: *hidden });
    info!(hidden = *hidden, event = "overlay_panic_toggled");
}
