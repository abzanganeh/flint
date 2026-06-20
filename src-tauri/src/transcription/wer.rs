//! Word error rate (WER) utilities for mic calibration scoring.

/// Normalise text for WER comparison: lowercase, strip punctuation, collapse whitespace.
pub fn normalize_for_wer(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            prev_space = false;
        } else if ch.is_whitespace() && !prev_space && !out.is_empty() {
            out.push(' ');
            prev_space = true;
        }
    }

    out.trim().to_string()
}

/// Token-level word error rate: Levenshtein distance / reference word count.
///
/// Returns `0.0` when the reference is empty (no words to compare).
pub fn word_error_rate(reference: &str, hypothesis: &str) -> f32 {
    let ref_norm = normalize_for_wer(reference);
    let ref_tokens: Vec<&str> = ref_norm.split_whitespace().collect();
    if ref_tokens.is_empty() {
        return 0.0;
    }

    let hyp_norm = normalize_for_wer(hypothesis);
    let hyp_tokens: Vec<&str> = hyp_norm.split_whitespace().collect();

    let distance = word_edit_distance(&ref_tokens, &hyp_tokens);
    distance as f32 / ref_tokens.len() as f32
}

#[allow(clippy::needless_range_loop)]
fn word_edit_distance(a: &[&str], b: &[&str]) -> usize {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![0usize; (m + 1) * (n + 1)];

    for i in 0..=m {
        dp[i * (n + 1)] = i;
    }
    for j in 0..=n {
        dp[j] = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let idx = i * (n + 1) + j;
            let replace = dp[(i - 1) * (n + 1) + (j - 1)] + cost;
            let delete = dp[(i - 1) * (n + 1) + j] + 1;
            let insert = dp[i * (n + 1) + (j - 1)] + 1;
            dp[idx] = replace.min(delete).min(insert);
        }
    }

    dp[m * (n + 1) + n]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PARAGRAPH: &str =
        "At SecureAuth, I led the design of an adaptive authentication system \
        using ML-based risk scoring. The platform supported OAuth 2.0 and OIDC federation across \
        multi-tenant SaaS customers. I integrated step-up MFA triggers with identity-aware policy \
        enforcement including Kerberos and LDAP for enterprise directories. My most recent work at \
        IdMe24 focused on agentic AI identity autonomous agents requiring just-in-time credential \
        provisioning with zero-standing privilege.";

    #[test]
    fn perfect_match_is_zero() {
        assert_eq!(word_error_rate(SAMPLE_PARAGRAPH, SAMPLE_PARAGRAPH), 0.0);
    }

    #[test]
    fn empty_reference_is_zero() {
        assert_eq!(word_error_rate("", "some words"), 0.0);
    }

    #[test]
    fn deliberate_typos_produce_nonzero_wer() {
        let hypothesis = "At SecureAuth I led the design of an adaptive authentication system \
            using ML based risk scoring. The platform supported OAuth 2 and OIDC federation across \
            multi tenant SaaS customers. I integrated step up MFA triggers with identity aware policy \
            enforcement including Kerberos and LDAP for enterprise directories. My most recent work at \
            IdMe 24 focused on agentic AI identity autonomous agents requiring just in time credential \
            provisioning with zero standing privilege.";
        let wer = word_error_rate(SAMPLE_PARAGRAPH, hypothesis);
        assert!(wer > 0.0 && wer < 0.30, "expected moderate WER, got {wer}");
    }

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(normalize_for_wer("OAuth 2.0, OIDC!"), "oauth 20 oidc");
    }

    #[test]
    fn completely_wrong_transcript_is_high_wer() {
        let wer = word_error_rate(SAMPLE_PARAGRAPH, "hello world foo bar");
        assert!(wer > 0.8, "expected high WER, got {wer}");
    }
}
