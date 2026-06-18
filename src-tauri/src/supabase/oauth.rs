//! Supabase GoTrue OAuth (PKCE) for desktop deep-link callback.
//!
//! Redirect URI registered in Supabase dashboard: `flint://auth/callback`

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use getrandom::getrandom;
use sha2::{Digest, Sha256};

use crate::interfaces::auth::AuthToken;
use chrono::{Duration as ChronoDuration, Utc};
use secrecy::SecretString;

use super::auth::SupabaseAuth;

/// OAuth redirect registered with Supabase + Google Cloud console.
pub const OAUTH_REDIRECT_URI: &str = "flint://auth/callback";

fn encode_redirect_uri(uri: &str) -> String {
    uri.replace(':', "%3A").replace('/', "%2F")
}

/// PKCE pair: `(code_verifier, code_challenge)`.
pub fn generate_pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    getrandom(&mut bytes).expect("OS RNG available");
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = code_challenge(&verifier);
    (verifier, challenge)
}

fn code_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

/// Parsed authorization code from `flint://auth/callback?code=…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCallbackCode {
    pub code: String,
}

/// Implicit/hash fallback when the provider returns tokens in the fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCallbackTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthCallback {
    Code(AuthCallbackCode),
    Tokens(AuthCallbackTokens),
    Error { message: String },
}

/// Parse `flint://auth/callback` query or hash into an OAuth result.
pub fn parse_auth_callback(url: &str) -> Option<AuthCallback> {
    let trimmed = url.trim();
    if !trimmed.starts_with("flint://") {
        return None;
    }
    let rest = trimmed.strip_prefix("flint://")?;
    let (host, tail) = match rest.split_once('#') {
        Some((h, fragment)) => (h, format!("#{fragment}")),
        None => {
            let (h, query) = match rest.split_once('?') {
                Some(parts) => parts,
                None => (rest, ""),
            };
            (
                h,
                if query.is_empty() {
                    String::new()
                } else {
                    format!("?{query}")
                },
            )
        }
    };

    if host != "auth/callback" && !host.starts_with("auth/callback/") {
        return None;
    }

    if let Some(fragment) = tail.strip_prefix('#') {
        return parse_hash_fragment(fragment);
    }

    let query = tail.strip_prefix('?').unwrap_or(&tail);
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key == "error" {
            let desc = query_param(query, "error_description").unwrap_or(value);
            return Some(AuthCallback::Error {
                message: url_decode(desc),
            });
        }
        if key == "code" {
            let code = url_decode(value);
            if !code.is_empty() {
                return Some(AuthCallback::Code(AuthCallbackCode { code }));
            }
        }
    }
    None
}

fn parse_hash_fragment(fragment: &str) -> Option<AuthCallback> {
    let mut access_token: Option<String> = None;
    let mut refresh_token: Option<String> = None;
    let mut expires_in: i64 = 3600;

    for pair in fragment.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "access_token" => access_token = Some(url_decode(value)),
            "refresh_token" => refresh_token = Some(url_decode(value)),
            "expires_in" => {
                expires_in = url_decode(value).parse().unwrap_or(3600);
            }
            "error" => {
                let desc = query_param(fragment, "error_description").unwrap_or(value);
                return Some(AuthCallback::Error {
                    message: url_decode(desc),
                });
            }
            _ => {}
        }
    }

    match (access_token, refresh_token) {
        (Some(access_token), Some(refresh_token)) => {
            Some(AuthCallback::Tokens(AuthCallbackTokens {
                access_token,
                refresh_token,
                expires_in,
            }))
        }
        _ => None,
    }
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == name {
                return Some(value);
            }
        }
    }
    None
}

fn url_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

impl SupabaseAuth {
    /// Build the browser URL that starts Google OAuth (PKCE).
    pub fn google_authorize_url(&self, code_challenge: &str) -> String {
        format!(
            "{}/auth/v1/authorize?provider=google&redirect_to={}&code_challenge={}&code_challenge_method=s256",
            self.public_base_url(),
            encode_redirect_uri(OAUTH_REDIRECT_URI),
            code_challenge
        )
    }
}

pub fn tokens_to_auth_token(tokens: &AuthCallbackTokens) -> AuthToken {
    let expires_at = Utc::now() + ChronoDuration::seconds(tokens.expires_in);
    AuthToken {
        access_token: SecretString::new(tokens.access_token.clone()),
        refresh_token: SecretString::new(tokens.refresh_token.clone()),
        expires_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pkce_callback_code() {
        let parsed = parse_auth_callback("flint://auth/callback?code=abc123&state=ignored");
        assert_eq!(
            parsed,
            Some(AuthCallback::Code(AuthCallbackCode {
                code: "abc123".into()
            }))
        );
    }

    #[test]
    fn parses_oauth_error() {
        let parsed = parse_auth_callback(
            "flint://auth/callback?error=access_denied&error_description=User%20cancelled",
        );
        assert!(matches!(parsed, Some(AuthCallback::Error { .. })));
    }

    #[test]
    fn parses_hash_fragment_tokens() {
        let parsed = parse_auth_callback(
            "flint://auth/callback#access_token=at&refresh_token=rt&expires_in=7200",
        );
        assert_eq!(
            parsed,
            Some(AuthCallback::Tokens(AuthCallbackTokens {
                access_token: "at".into(),
                refresh_token: "rt".into(),
                expires_in: 7200,
            }))
        );
    }

    #[test]
    fn rejects_import_host() {
        assert!(parse_auth_callback("flint://import?token=abc").is_none());
    }

    #[test]
    fn pkce_challenge_is_deterministic() {
        let challenge = code_challenge("test-verifier");
        assert!(!challenge.is_empty());
        assert_eq!(challenge, code_challenge("test-verifier"));
    }
}
