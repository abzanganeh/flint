//! Parse `flint://` deep-link URLs for Smart Resume import.

use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Extract the handoff token from `flint://import?token=<uuid>`.
pub fn parse_import_token(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if !trimmed.starts_with("flint://") {
        return None;
    }

    let rest = trimmed.strip_prefix("flint://")?;
    let (host, query) = match rest.split_once('?') {
        Some((h, q)) => (h, q),
        None => (rest, ""),
    };

    if host != "import" && !host.starts_with("import/") {
        return None;
    }

    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key == "token" {
            let token = value.trim();
            if token.is_empty() || token.len() > 64 {
                return None;
            }
            return Some(token.to_string());
        }
    }
    None
}

/// Bring the main window to the foreground after a Smart Resume handoff.
///
/// The overlay may be hidden via panic-hide (Ctrl+Alt+Shift) or sit behind the
/// browser when a second instance forwards a deep link to an existing process.
pub fn present_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window("main") else {
        tracing::warn!("present_main_window: main window not found");
        return;
    };
    if let Err(e) = win.unminimize() {
        tracing::warn!(error = %e, "present_main_window: unminimize failed");
    }
    if let Err(e) = win.show() {
        tracing::warn!(error = %e, "present_main_window: show failed");
    }
    if let Err(e) = win.set_focus() {
        tracing::warn!(error = %e, "present_main_window: set_focus failed");
    }
}

/// Emit `smart_resume_import_token` when `url` is a valid import link.
pub fn emit_import_token_if_present<R: Runtime>(app: &AppHandle<R>, url: &str) -> bool {
    let Some(token) = parse_import_token(url) else {
        return false;
    };
    if let Err(e) = app.emit("smart_resume_import_token", token) {
        tracing::warn!(error = %e, "emit smart_resume_import_token failed");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_import_url() {
        let token = parse_import_token("flint://import?token=550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(
            token.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn rejects_missing_token() {
        assert!(parse_import_token("flint://import").is_none());
    }

    #[test]
    fn rejects_wrong_host() {
        assert!(parse_import_token("flint://other?token=abc").is_none());
    }

    #[test]
    fn rejects_empty_token() {
        assert!(parse_import_token("flint://import?token=").is_none());
    }

    #[test]
    fn extracts_token_when_bare_param_present() {
        // A bare query parameter (no `=`) must not shadow a valid token param.
        let token = parse_import_token(
            "flint://import?someFlag&token=550e8400-e29b-41d4-a716-446655440000",
        );
        assert_eq!(
            token.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }
}
