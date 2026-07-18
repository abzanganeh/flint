//! Byte-level line buffering for SSE/NDJSON provider streams.
//!
//! `reqwest::Response::bytes_stream()` yields raw network chunks — one chunk
//! does **not** guarantee one complete `data: {...}` line. Every provider
//! today calls `.lines()` independently per chunk with no carried-over
//! buffer, so a line split across two chunks silently drops both halves
//! (the first half is incomplete JSON and fails to parse, the second half no
//! longer starts with `data: ` and also fails to parse). This module fixes
//! that by buffering at the byte level across chunk boundaries and only
//! decoding UTF-8 once a full line has been assembled.

use anyhow::Result;
use bytes::Bytes;
use futures::stream::{self, Stream};
use futures::StreamExt;

const NEWLINE: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';

/// State carried across `stream::unfold` polls: the source byte stream, the
/// bytes accumulated since the last complete line, and whether the source
/// has already ended (so remaining bytes can still be flushed as a final
/// line).
struct BufferedLinesState<S> {
    byte_stream: S,
    buffer: Vec<u8>,
    stream_ended: bool,
}

/// Buffers a byte stream and yields complete newline-terminated lines,
/// carrying partial lines (and partial multi-byte UTF-8 sequences) across
/// chunk boundaries so a `data: {...}` line split across two network reads
/// is never silently dropped.
pub fn buffered_lines<S>(byte_stream: S) -> impl Stream<Item = Result<String>>
where
    S: Stream<Item = Result<Bytes>> + Unpin,
{
    let initial_state = BufferedLinesState {
        byte_stream,
        buffer: Vec::new(),
        stream_ended: false,
    };

    stream::unfold(initial_state, |mut state| async move {
        loop {
            if let Some(line) = take_complete_line(&mut state.buffer) {
                if let Some(text) = decode_non_empty_line(line) {
                    return Some((Ok(text), state));
                }
                continue;
            }

            if state.stream_ended {
                return take_final_line(&mut state.buffer).map(|text| (Ok(text), state));
            }

            match state.byte_stream.next().await {
                Some(Ok(chunk)) => state.buffer.extend_from_slice(&chunk),
                Some(Err(err)) => return Some((Err(err), state)),
                None => state.stream_ended = true,
            }
        }
    })
}

/// Removes and returns the first complete line (excluding the terminating
/// `\n`, and a preceding `\r` if present) from `buffer`, if one exists yet.
fn take_complete_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let newline_pos = buffer.iter().position(|&byte| byte == NEWLINE)?;
    let mut line: Vec<u8> = buffer.drain(..=newline_pos).collect();
    line.pop();
    if line.last() == Some(&CARRIAGE_RETURN) {
        line.pop();
    }
    Some(line)
}

/// Flushes whatever remains in `buffer` once the source stream has ended —
/// a response with no trailing newline still has a final line to yield.
fn take_final_line(buffer: &mut Vec<u8>) -> Option<String> {
    let remaining = std::mem::take(buffer);
    decode_non_empty_line(remaining)
}

/// Decodes bytes to UTF-8 only once a full line has been assembled (never
/// per-chunk, so a multi-byte character split across chunks decodes
/// correctly), and filters out empty/whitespace-only lines to match
/// existing provider behavior.
fn decode_non_empty_line(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn ok_stream(chunks: Vec<&str>) -> impl Stream<Item = Result<Bytes>> + Unpin {
        stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok(Bytes::from(chunk.as_bytes().to_vec())))
                .collect::<Vec<_>>(),
        )
    }

    async fn collect_results(
        byte_stream: impl Stream<Item = Result<Bytes>> + Unpin,
    ) -> Vec<Result<String>> {
        buffered_lines(byte_stream).collect().await
    }

    #[tokio::test]
    async fn line_split_across_two_chunks_reassembles() {
        let chunks = vec![
            "data: {\"choices\":[{\"delta\":",
            "{\"content\":\"hi\"}}]}\n",
        ];
        let results = collect_results(ok_stream(chunks)).await;

        assert_eq!(results.len(), 1);
        let line = results[0].as_ref().expect("expected Ok line");
        assert_eq!(
            line,
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}"
        );
    }

    #[tokio::test]
    async fn multiple_complete_lines_in_one_chunk_yield_in_order() {
        let chunks = vec!["data: first\ndata: second\ndata: third\n"];
        let results = collect_results(ok_stream(chunks)).await;

        let lines: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(lines, vec!["data: first", "data: second", "data: third"]);
    }

    #[tokio::test]
    async fn empty_and_whitespace_only_lines_are_skipped() {
        let chunks = vec!["data: a\n\n   \ndata: b\n"];
        let results = collect_results(ok_stream(chunks)).await;

        let lines: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(lines, vec!["data: a", "data: b"]);
    }

    #[tokio::test]
    async fn stream_end_without_trailing_newline_flushes_last_line() {
        let chunks = vec!["data: complete\n", "data: no-newline-at-end"];
        let results = collect_results(ok_stream(chunks)).await;

        let lines: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(lines, vec!["data: complete", "data: no-newline-at-end"]);
    }

    #[tokio::test]
    async fn byte_stream_error_is_propagated_immediately() {
        let error_stream = stream::iter(vec![
            Ok(Bytes::from_static(b"data: one\n")),
            Err(anyhow!("simulated stream read error")),
        ]);
        let results = collect_results(error_stream).await;

        // The error surfaces right after the complete line preceding it,
        // rather than being dropped or deferred to the end of the stream.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap(), "data: one");
        assert_eq!(
            results[1].as_ref().unwrap_err().to_string(),
            "simulated stream read error"
        );
    }

    #[tokio::test]
    async fn multibyte_utf8_char_split_at_chunk_boundary_decodes_correctly() {
        // "café" — the 'é' is encoded as 0xC3 0xA9; split the two bytes of
        // that character across separate chunks.
        let full_line = "data: café\n";
        let full_bytes = full_line.as_bytes();
        let split_at = full_bytes
            .iter()
            .position(|&b| b == 0xC3)
            .expect("expected multi-byte UTF-8 lead byte")
            + 1;

        let first_chunk = Bytes::from(full_bytes[..split_at].to_vec());
        let second_chunk = Bytes::from(full_bytes[split_at..].to_vec());
        let chunk_stream = stream::iter(vec![Ok(first_chunk), Ok(second_chunk)]);

        let results = collect_results(chunk_stream).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), "data: café");
    }
}
