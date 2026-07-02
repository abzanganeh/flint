//! Human-readable session export formats (text + PDF) built on [`SessionExport`].

use std::io::{BufWriter, Cursor, Write};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use printpdf::{BuiltinFont, Mm, PdfDocument};

use super::persistence::{ResponseExport, SessionExport, TranscriptChunkExport};

const PDF_PAGE_WIDTH_MM: f32 = 210.0;
const PDF_PAGE_HEIGHT_MM: f32 = 297.0;
const PDF_LEFT_MARGIN_MM: f32 = 15.0;
const PDF_TOP_MM: f32 = 280.0;
const PDF_BOTTOM_MM: f32 = 15.0;
const PDF_LINE_HEIGHT_MM: f32 = 5.0;
const PDF_FONT_SIZE: f32 = 10.0;
const PDF_MAX_CHARS_PER_LINE: usize = 95;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExportFormat {
    Json,
    Text,
    Pdf,
}

impl SessionExportFormat {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "text" | "txt" => Ok(Self::Text),
            "pdf" => Ok(Self::Pdf),
            other => {
                anyhow::bail!("Unsupported export format '{other}' — expected json, text, or pdf")
            }
        }
    }
}

/// Suggested download filename for the export bundle.
pub fn export_filename(session: &SessionExport, format: SessionExportFormat) -> String {
    let slug = slugify(&session.name);
    let date = Utc
        .timestamp_opt(session.created_at, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown-date".to_string());
    let ext = match format {
        SessionExportFormat::Json => "json",
        SessionExportFormat::Text => "txt",
        SessionExportFormat::Pdf => "pdf",
    };
    format!("flint-session-{slug}-{date}.{ext}")
}

pub fn format_session_json(session: &SessionExport) -> Result<String> {
    serde_json::to_string_pretty(session).context("serialise session export as JSON")
}

pub fn format_session_text(session: &SessionExport) -> String {
    let mut out = String::new();
    out.push_str("Flint Session Export\n");
    out.push_str("====================\n\n");
    out.push_str(&format!("Session: {}\n", session.name));
    out.push_str(&format!("Type: {}\n", session.session_type));
    out.push_str(&format!("Domain: {}\n", session.domain));
    out.push_str(&format!("State: {}\n", session.state));
    out.push_str(&format!(
        "Created: {}\n",
        format_timestamp(session.created_at)
    ));
    out.push_str(&format!(
        "Expires: {}\n",
        format_timestamp(session.expires_at)
    ));
    out.push('\n');

    append_design_section(&mut out, session);
    append_transcript_section(&mut out, &session.transcript_chunks);
    append_responses_section(&mut out, &session.responses);

    out
}

pub fn format_session_pdf(session: &SessionExport) -> Result<Vec<u8>> {
    let body = format_session_text(session);
    render_pdf_bytes(&session.name, &body)
}

fn append_design_section(out: &mut String, session: &SessionExport) {
    let fields = [
        ("Job description", &session.job_description),
        ("Profile", &session.profile),
        ("Company overview", &session.company_overview),
        ("Leadership principles", &session.leadership_principles),
        ("Role expectations", &session.role_expectations),
        ("Technical prep", &session.technical_prep),
        ("Strategy notes", &session.strategy_notes),
    ];
    let mut any = false;
    for (label, value) in fields {
        if value.trim().is_empty() {
            continue;
        }
        if !any {
            out.push_str("--- Session design ---\n");
            any = true;
        }
        out.push_str(&format!("{label}:\n{value}\n\n"));
    }
    if !session.context_text.trim().is_empty() && session.job_description.trim().is_empty() {
        out.push_str("--- Context ---\n");
        out.push_str(&session.context_text);
        out.push_str("\n\n");
    }
}

fn append_transcript_section(out: &mut String, chunks: &[TranscriptChunkExport]) {
    out.push_str("--- Transcript ---\n");
    if chunks.is_empty() {
        out.push_str("(no transcript recorded)\n\n");
        return;
    }
    for utterance in merge_transcript_chunks(chunks) {
        out.push_str(&format!("{}: {}\n\n", utterance.label, utterance.text));
    }
}

fn append_responses_section(out: &mut String, responses: &[ResponseExport]) {
    out.push_str("--- AI responses ---\n");
    if responses.is_empty() {
        out.push_str("(no AI responses recorded)\n");
        return;
    }
    for response in responses {
        out.push_str(&format!(
            "[{}] confidence {:.0}%\n{}\n\n",
            response.response_type,
            response.confidence * 100.0,
            response.content.trim()
        ));
    }
}

struct TranscriptUtterance {
    label: &'static str,
    text: String,
}

fn merge_transcript_chunks(chunks: &[TranscriptChunkExport]) -> Vec<TranscriptUtterance> {
    let mut out: Vec<TranscriptUtterance> = Vec::new();
    for chunk in chunks {
        let label = speaker_label(&chunk.speaker);
        if let Some(last) = out.last_mut() {
            if last.label == label {
                if !last.text.is_empty() {
                    last.text.push(' ');
                }
                last.text.push_str(chunk.text.trim());
                continue;
            }
        }
        out.push(TranscriptUtterance {
            label,
            text: chunk.text.trim().to_string(),
        });
    }
    out
}

fn speaker_label(raw: &str) -> &'static str {
    match raw {
        "System" => "INTERVIEWER",
        "Microphone" => "YOU",
        _ => "SPEAKER",
    }
}

fn format_timestamp(secs: i64) -> String {
    Utc.timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn render_pdf_bytes(title: &str, body: &str) -> Result<Vec<u8>> {
    let (doc, first_page, first_layer) = PdfDocument::new(
        title,
        Mm(PDF_PAGE_WIDTH_MM),
        Mm(PDF_PAGE_HEIGHT_MM),
        "Layer 1",
    );
    let font = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .context("load PDF built-in font")?;

    let mut page = first_page;
    let mut layer = first_layer;
    let mut y = PDF_TOP_MM;

    for line in body.lines() {
        for wrapped in wrap_line(line, PDF_MAX_CHARS_PER_LINE) {
            if y < PDF_BOTTOM_MM {
                let (next_page, next_layer) =
                    doc.add_page(Mm(PDF_PAGE_WIDTH_MM), Mm(PDF_PAGE_HEIGHT_MM), "Layer 1");
                page = next_page;
                layer = next_layer;
                y = PDF_TOP_MM;
            }
            doc.get_page(page).get_layer(layer).use_text(
                sanitize_pdf_text(&wrapped),
                PDF_FONT_SIZE,
                Mm(PDF_LEFT_MARGIN_MM),
                Mm(y),
                &font,
            );
            y -= PDF_LINE_HEIGHT_MM;
        }
    }

    let mut buffer = BufWriter::new(Cursor::new(Vec::new()));
    doc.save(&mut buffer).context("write PDF export bytes")?;
    buffer.flush().context("flush PDF export bytes")?;
    Ok(buffer
        .into_inner()
        .map_err(|_| anyhow::anyhow!("PDF export buffer unavailable after flush"))?
        .into_inner())
}

fn wrap_line(line: &str, max_chars: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<char> = line.chars().collect();
    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        let mut break_at = end;
        if end < chars.len() {
            if let Some(space) = chars[start..end].iter().rposition(|c| c.is_whitespace()) {
                break_at = start + space;
            }
        }
        if break_at <= start {
            break_at = end;
        }
        out.push(
            chars[start..break_at]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        );
        start = if break_at < chars.len() && chars[break_at].is_whitespace() {
            break_at + 1
        } else {
            break_at
        };
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn sanitize_pdf_text(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .collect()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::session::persistence::{ResponseExport, SessionExport, TranscriptChunkExport};

    fn sample_session() -> SessionExport {
        SessionExport {
            id: Uuid::new_v4(),
            state: "ENDED".to_string(),
            created_at: 1_700_000_000,
            expires_at: 1_700_086_400,
            promoted: false,
            name: "Staff IAM".to_string(),
            session_type: "interview".to_string(),
            domain: "iam".to_string(),
            context_text: String::new(),
            job_description: "Lead identity platform work.".to_string(),
            profile: String::new(),
            company_overview: String::new(),
            leadership_principles: String::new(),
            role_expectations: String::new(),
            technical_prep: String::new(),
            strategy_notes: String::new(),
            transcript_chunks: vec![
                TranscriptChunkExport {
                    id: "c1".to_string(),
                    speaker: "System".to_string(),
                    text: "Tell me about yourself.".to_string(),
                    timestamp_ms: 0,
                    created_at: 0,
                },
                TranscriptChunkExport {
                    id: "c2".to_string(),
                    speaker: "Microphone".to_string(),
                    text: "I build IAM platforms.".to_string(),
                    timestamp_ms: 1,
                    created_at: 1,
                },
            ],
            responses: vec![ResponseExport {
                id: "r1".to_string(),
                response_type: "directional".to_string(),
                content: "Mention Okta and SailPoint.".to_string(),
                confidence: 0.92,
                created_at: 2,
            }],
            state_transitions: vec![],
        }
    }

    #[test]
    fn text_export_includes_transcript_and_responses() {
        let text = format_session_text(&sample_session());
        assert!(text.contains("Staff IAM"));
        assert!(text.contains("INTERVIEWER: Tell me about yourself."));
        assert!(text.contains("YOU: I build IAM platforms."));
        assert!(text.contains("[directional] confidence 92%"));
        assert!(text.contains("Lead identity platform work."));
    }

    #[test]
    fn json_export_round_trips() {
        let session = sample_session();
        let json = format_session_json(&session).expect("json export");
        assert!(json.contains("\"name\": \"Staff IAM\""));
    }

    #[test]
    fn pdf_export_produces_non_empty_bytes() {
        let pdf = format_session_pdf(&sample_session()).expect("pdf export");
        assert!(pdf.starts_with(b"%PDF"));
        assert!(pdf.len() > 100);
    }

    #[test]
    fn export_filename_slugifies_session_name() {
        let filename = export_filename(&sample_session(), SessionExportFormat::Text);
        assert_eq!(filename, "flint-session-staff-iam-2023-11-14.txt");
    }

    #[test]
    fn merge_transcript_chunks_combines_same_speaker() {
        let merged = merge_transcript_chunks(&[
            TranscriptChunkExport {
                id: "a".to_string(),
                speaker: "System".to_string(),
                text: "Hello".to_string(),
                timestamp_ms: 0,
                created_at: 0,
            },
            TranscriptChunkExport {
                id: "b".to_string(),
                speaker: "System".to_string(),
                text: "there".to_string(),
                timestamp_ms: 1,
                created_at: 1,
            },
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "Hello there");
    }
}
