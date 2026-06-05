//! Parse `flint://` deep-link URLs for Smart Resume import.

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
