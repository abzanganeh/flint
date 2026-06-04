//! Simple word-based text chunker used before `ingest_context` embeds and
//! stores context into the vector store.
//!
//! A proper sub-word tokeniser would give exact token counts, but fastembed
//! accepts raw text so approximating at 1 word ≈ 1.33 tokens is sufficient.
//! The chunk + overlap sizes in the public API are expressed in *tokens* and
//! converted to words internally.
//!
//! Reference: design doc §11 (RAG ingestion — 200-token chunks, 50-token
//! overlap), Task 2.11 spec.

#![allow(dead_code)]

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

/// Approximate token-to-word ratio for English text with sub-word tokenisers.
const TOKENS_PER_WORD: f32 = 1.33;

fn tokens_to_words(tokens: usize) -> usize {
    ((tokens as f32) / TOKENS_PER_WORD).round() as usize
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Split `text` into overlapping chunks suitable for embedding.
///
/// * `chunk_size_tokens` — target chunk length in tokens (e.g. 200).
/// * `overlap_tokens` — overlap between consecutive chunks in tokens
///   (e.g. 50).
///
/// The returned strings are owned slices of whitespace-normalised words.
/// Trailing empty chunks are never returned.
pub fn chunk_text(text: &str, chunk_size_tokens: usize, overlap_tokens: usize) -> Vec<String> {
    let chunk_words = tokens_to_words(chunk_size_tokens).max(1);
    let overlap_words = tokens_to_words(overlap_tokens);
    let step = chunk_words.saturating_sub(overlap_words).max(1);

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < words.len() {
        let end = (start + chunk_words).min(words.len());
        let chunk = words[start..end].join(" ");
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        if end == words.len() {
            break;
        }
        start += step;
    }

    chunks
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text_returns_no_chunks() {
        assert!(chunk_text("", 200, 50).is_empty());
        assert!(chunk_text("   ", 200, 50).is_empty());
    }

    #[test]
    fn test_short_text_returns_one_chunk() {
        let text = "hello world";
        let chunks = chunk_text(text, 200, 50);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }

    #[test]
    fn test_chunk_sizes_are_bounded() {
        // ~300 words → should produce multiple chunks with chunk_size=200 tokens
        let words: Vec<String> = (0..300).map(|i| format!("word{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text, 200, 50);
        let max_words = tokens_to_words(200) + 2; // small tolerance
        for chunk in &chunks {
            let word_count = chunk.split_whitespace().count();
            assert!(
                word_count <= max_words,
                "chunk has {word_count} words, expected ≤ {max_words}"
            );
        }
    }

    #[test]
    fn test_overlap_is_present() {
        // 200 words → 2 chunks with overlap the last words of chunk 1 appear
        // in the first words of chunk 2.
        let words: Vec<String> = (0..200).map(|i| format!("w{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_text(&text, 100, 25); // smaller for speed
        assert!(chunks.len() >= 2, "expected at least 2 chunks");

        // The last word of chunk[0] must appear in chunk[1] (overlap check).
        let last_of_first = chunks[0].split_whitespace().last().unwrap();
        assert!(
            chunks[1].contains(last_of_first),
            "overlap: last word of chunk 0 ({last_of_first:?}) not found in chunk 1"
        );
    }

    #[test]
    fn test_all_words_appear_in_at_least_one_chunk() {
        let words: Vec<&str> = vec!["alpha", "beta", "gamma", "delta", "epsilon"];
        let text = words.join(" ");
        let chunks = chunk_text(&text, 3, 1);
        let combined = chunks.join(" ");
        for word in &words {
            assert!(
                combined.contains(word),
                "word {word:?} missing from all chunks"
            );
        }
    }
}
