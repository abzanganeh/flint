//! Platform-specific stealth helpers: capture exclusion + multi-monitor
//! placement.
//!
//! References:
//! - `.cursor/rules/flint-security.mdc` — Stealth Mode Requirements
//! - `flint_system_design_v3.md` §17 — Stealth & Screen Capture
//!
//! Capture exclusion is best-effort and OS-dependent. The X11 path is hard-
//! failed at health-check time so this module assumes Wayland on Linux. On
//! Wayland there is no standardised compositor exclusion protocol, so we log
//! and rely on the X11 hard fail plus the user-controlled PipeWire portal.
//!
//! macOS exclusion (`NSWindow.sharingType = NSWindowSharingNone`) is wired via
//! raw `objc_msgSend` FFI to avoid a new crate dependency. The Objective-C
//! runtime is always present in any AppKit process, so no extra linker flags
//! are required.

use tauri::{AppHandle, Manager, Runtime};
use tracing::{info, warn};

/// Apply OS-level capture exclusion to the main overlay window.
///
/// Idempotent; safe to call on every platform.
pub fn apply_capture_exclusion<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        warn!("apply_capture_exclusion: main window not found");
        return;
    };

    apply_capture_exclusion_impl(&window);
}

#[cfg(target_os = "windows")]
fn apply_capture_exclusion_impl<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    // Bind the Win32 API directly via `extern "system"` to avoid pulling in
    // the `windows` / `windows-sys` crates as a build-time dependency on
    // non-Windows hosts. HWND is `*mut c_void` in both Tauri's `windows`
    // re-export and the raw Win32 ABI; we transmute the repr(transparent)
    // wrapper into a raw pointer for the call.
    use std::ffi::c_void;

    type RawHwnd = *mut c_void;
    type Bool = i32;
    type Dword = u32;

    const WDA_EXCLUDEFROMCAPTURE: Dword = 0x0000_0011;

    extern "system" {
        fn SetWindowDisplayAffinity(hwnd: RawHwnd, affinity: Dword) -> Bool;
    }

    let hwnd_struct = match window.hwnd() {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "capture exclusion: hwnd unavailable");
            return;
        }
    };

    // SAFETY: `tauri`'s HWND is a `#[repr(transparent)]` newtype over
    // `*mut c_void`. The transmute reinterprets the same underlying pointer
    // value, no aliasing or lifetime change.
    let raw: RawHwnd = unsafe { std::mem::transmute(hwnd_struct) };

    // SAFETY: `SetWindowDisplayAffinity` is a thread-safe Win32 API that
    // accepts any HWND owned by the current process.
    let ok = unsafe { SetWindowDisplayAffinity(raw, WDA_EXCLUDEFROMCAPTURE) };
    if ok == 0 {
        warn!("capture exclusion: SetWindowDisplayAffinity returned 0");
    } else {
        info!("capture exclusion applied (WDA_EXCLUDEFROMCAPTURE)");
    }
}

#[cfg(target_os = "macos")]
fn apply_capture_exclusion_impl<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    // Raw `objc_msgSend` FFI to keep dependencies minimal.
    // `NSWindowSharingNone = 0` instructs AppKit's window server to exclude
    // the window from screen capture (CGWindowList / ScreenCaptureKit / OBS).
    use std::ffi::{c_char, c_void};

    type Sel = *mut c_void;
    type Object = *mut c_void;

    const NS_WINDOW_SHARING_NONE: usize = 0;

    extern "C" {
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }

    let ns_window_ptr = match window.ns_window() {
        Ok(ptr) => ptr as Object,
        Err(e) => {
            warn!(error = %e, "capture exclusion: ns_window unavailable");
            return;
        }
    };
    if ns_window_ptr.is_null() {
        warn!("capture exclusion: ns_window is null");
        return;
    }

    // SAFETY: `b"setSharingType:\0"` is a valid C string; `sel_registerName`
    // returns a stable, process-lifetime selector.
    let sel = unsafe { sel_registerName(b"setSharingType:\0".as_ptr() as *const c_char) };

    // SAFETY: `objc_msgSend` is an untyped ABI trampoline. We transmute it to
    // the exact signature for `-[NSWindow setSharingType:]` so the calling
    // convention and argument count match the Objective-C method.
    let send: unsafe extern "C" fn(Object, Sel, usize) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    // SAFETY: `ns_window_ptr` is a valid NSWindow vended by Tauri.
    unsafe { send(ns_window_ptr, sel, NS_WINDOW_SHARING_NONE) };
    info!("capture exclusion applied (NSWindowSharingNone)");
}

#[cfg(all(unix, not(target_os = "macos")))]
fn apply_capture_exclusion_impl<R: Runtime>(_window: &tauri::WebviewWindow<R>) {
    // Wayland has no portable compositor exclusion protocol; the X11 path is
    // hard-failed in `health/checks.rs::check_stealth_api`. The user-facing
    // protection on Wayland is the PipeWire portal, which is per-source and
    // user-controlled — there is nothing to call at the Rust level.
    info!(
        "capture exclusion: Wayland — relying on X11 hard-fail at health \
         check and on the PipeWire portal source picker"
    );
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn apply_capture_exclusion_impl<R: Runtime>(_window: &tauri::WebviewWindow<R>) {
    warn!("capture exclusion: unsupported target OS");
}

/// Move the overlay to a non-primary monitor when more than one is connected.
///
/// `.cursor/rules/flint-security.mdc` requires defaulting to a non-primary
/// display when available; positions on the chosen monitor are top-right with
/// a 40px inset so the overlay sits out of the way of typical video tiles.
///
/// Unused in debug builds — [`configure_dev_window`] centres on the primary
/// display instead.
#[cfg_attr(debug_assertions, allow(dead_code))]
pub fn place_on_non_primary_monitor<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        warn!("place_on_non_primary_monitor: main window not found");
        return;
    };

    let monitors = match window.available_monitors() {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "place_on_non_primary_monitor: enumeration failed");
            return;
        }
    };

    if monitors.len() < 2 {
        return;
    }

    let primary = match window.primary_monitor() {
        Ok(Some(m)) => m,
        _ => return,
    };

    let primary_pos = primary.position();
    let target = monitors.iter().find(|m| m.position() != primary_pos);
    let Some(target) = target else {
        return;
    };

    let pos = target.position();
    let size = target.size();
    let win_size = window.outer_size().unwrap_or(tauri::PhysicalSize {
        width: 480,
        height: 720,
    });

    let inset_x = pos.x + (size.width as i32) - (win_size.width as i32) - 40;
    let inset_y = pos.y + 40;
    let new_pos = tauri::PhysicalPosition {
        x: inset_x.max(pos.x),
        y: inset_y,
    };

    if let Err(e) = window.set_position(new_pos) {
        warn!(error = %e, "place_on_non_primary_monitor: set_position failed");
    } else {
        info!(
            x = new_pos.x,
            y = new_pos.y,
            "overlay placed on non-primary monitor"
        );
    }
}

/// Dev-only window setup: draggable chrome, taskbar entry, primary monitor.
///
/// Release builds keep frameless always-on-top placement on the non-primary
/// display (stealth overlay). Debug builds are easier to move and find.
#[cfg(debug_assertions)]
pub fn configure_dev_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        warn!("configure_dev_window: main window not found");
        return;
    };

    // Keep the window frameless even in dev — the React TitleBar provides
    // drag, minimize, maximize, and close. Enabling OS decorations here caused
    // non-functional buttons on Wayland because the hint arrives after mapping.
    if let Err(e) = window.set_always_on_top(false) {
        warn!(error = %e, "configure_dev_window: set_always_on_top failed");
    }
    if let Err(e) = window.set_skip_taskbar(false) {
        warn!(error = %e, "configure_dev_window: set_skip_taskbar failed");
    }

    place_on_primary_monitor_centred(&window);
    info!("dev window: decorations on, centred on primary monitor");
}

#[cfg(debug_assertions)]
fn place_on_primary_monitor_centred<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let Some(primary) = window.primary_monitor().ok().flatten() else {
        return;
    };

    let pos = primary.position();
    let size = primary.size();
    let win_size = window.outer_size().unwrap_or(tauri::PhysicalSize {
        width: 800,
        height: 600,
    });

    let x = pos.x + (size.width as i32 - win_size.width as i32) / 2;
    let y = pos.y + (size.height as i32 - win_size.height as i32) / 2;
    let new_pos = tauri::PhysicalPosition {
        x: x.max(pos.x),
        y: y.max(pos.y),
    };

    if let Err(e) = window.set_position(new_pos) {
        warn!(error = %e, "place_on_primary_monitor_centred: set_position failed");
    }
}
