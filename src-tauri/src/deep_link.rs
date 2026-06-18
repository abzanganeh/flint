//! Parse `flint://` deep-link URLs for Smart Resume import.

use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Scan process arguments and the `FLINT_IMPORT_URL` env variable to capture
/// a cold-start import token before the React WebView mounts.
///
/// Priority: first matching CLI arg wins; env var is the fallback for dev
/// workflows where the shell handler passes the URL via the environment.
pub fn capture_cold_start_token() -> Option<String> {
    for arg in std::env::args().skip(1) {
        if let Some(t) = parse_import_token(&arg) {
            return Some(t);
        }
    }
    capture_cold_start_token_from_env()
}

/// Check only `FLINT_IMPORT_URL` for a cold-start token (no argv scan).
///
/// Useful in tests where `std::env::args()` reflects the test runner's own
/// arguments rather than a `flint://` deep link.
pub fn capture_cold_start_token_from_env() -> Option<String> {
    let url = std::env::var("FLINT_IMPORT_URL").ok()?;
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    parse_import_token(trimmed)
}

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

/// Spawn async OAuth token exchange when `url` is `flint://auth/callback`.
pub fn spawn_oauth_callback_if_present<R: Runtime>(app: &AppHandle<R>, url: &str) -> bool {
    if crate::supabase::oauth::parse_auth_callback(url).is_none() {
        return false;
    }
    let app = app.clone();
    let url = url.to_string();
    tauri::async_runtime::spawn(async move {
        let _ = crate::commands::process_oauth_callback_url(&app, &url).await;
    });
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

    #[test]
    fn accepts_import_subpath() {
        // `flint://import/` (with trailing slash) should still be accepted.
        let token =
            parse_import_token("flint://import/?token=550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(
            token.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn rejects_token_over_64_chars() {
        // Token beyond 64 chars must be rejected.
        let long = "a".repeat(65);
        let url = format!("flint://import?token={long}");
        assert!(parse_import_token(&url).is_none());
    }

    #[test]
    fn accepts_token_exactly_64_chars() {
        let token_64 = "a".repeat(64);
        let url = format!("flint://import?token={token_64}");
        assert_eq!(parse_import_token(&url).as_deref(), Some(token_64.as_str()));
    }

    #[test]
    fn token_with_trailing_whitespace_stripped() {
        // URLs coming from xdg-open on some distros may carry a trailing newline.
        let token =
            parse_import_token("  flint://import?token=550e8400-e29b-41d4-a716-446655440000  ");
        assert_eq!(
            token.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn last_token_param_wins_when_duplicated() {
        // If two `token=` params are present, first match wins (predictable behaviour).
        let token = parse_import_token(
            "flint://import?token=first-token-00000000000000000000000000&token=second",
        );
        assert_eq!(
            token.as_deref(),
            Some("first-token-00000000000000000000000000")
        );
    }

    #[test]
    fn rejects_non_flint_scheme() {
        assert!(parse_import_token("https://import?token=abc").is_none());
        assert!(parse_import_token("http://flint?token=abc").is_none());
    }
}
