//! Extract structured JSON from LLM post-session summary output.

/// Return the first `{ … }` slice with string-aware brace matching.
pub fn extract_balanced_json(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escape = false;

    for i in start..bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_markdown_fence(raw: &str) -> &str {
    let stripped = raw
        .strip_prefix("```json")
        .or_else(|| raw.strip_prefix("```"))
        .unwrap_or(raw);
    stripped.strip_suffix("```").unwrap_or(stripped).trim()
}

/// Pull a JSON object string from noisy LLM output (preamble, fences, suffix).
pub fn extract_json_object(raw: &str) -> Option<String> {
    let trimmed = strip_markdown_fence(raw.trim());
    extract_balanced_json(trimmed)
}

/// Parse LLM summary text into JSON string, or return `fallback` if unparseable.
pub fn normalize_llm_summary_json(raw: &str, fallback: &str) -> String {
    if let Some(slice) = extract_json_object(raw) {
        if serde_json::from_str::<serde_json::Value>(&slice).is_ok() {
            return slice;
        }
    }
    if serde_json::from_str::<serde_json::Value>(raw.trim()).is_ok() {
        return raw.trim().to_string();
    }
    fallback.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_markdown_fence() {
        let raw = r#"Here is the summary:
```json
{"one_line_summary":"Good session","questions_count":0}
```
Thanks!"#;
        let out = extract_json_object(raw).expect("json");
        assert!(out.contains("Good session"));
    }

    #[test]
    fn extracts_json_with_preamble() {
        let raw = r#"Summary follows: {"topics_covered":["rust"],"one_line_summary":"ok"}"#;
        let out = extract_json_object(raw).expect("json");
        assert_eq!(
            out,
            r#"{"topics_covered":["rust"],"one_line_summary":"ok"}"#
        );
    }

    #[test]
    fn normalize_falls_back_on_prose() {
        let fallback = r#"{"one_line_summary":"fallback"}"#;
        let out = normalize_llm_summary_json("Not JSON at all.", fallback);
        assert_eq!(out, fallback);
    }
}
