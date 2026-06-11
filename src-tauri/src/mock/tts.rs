//! Platform TTS abstraction — Phase 1 uses OS-provided speech synthesis.
//!
//! macOS   → `say -r 160 "<text>"`
//! Linux   → `espeak-ng "<text>"` (falls back to `espeak` if ng is absent)
//! Windows → PowerShell `Add-Type -A System.Speech; …SpeechSynthesizer`
//!
//! Each call spawns a child process and awaits completion so the conductor
//! can await it before recording the user's answer.  The overhead is
//! typically <50 ms for process launch + TTS latency.

use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{instrument, warn};

/// Speak `text` using the platform TTS engine and wait for it to finish.
///
/// Silently truncates to 500 characters to avoid runaway TTS on malformed
/// LLM output.
#[instrument(skip(text), fields(len = text.len()))]
pub async fn speak(text: &str) -> Result<()> {
    let text = truncate_for_tts(text);
    run_tts_command(&text).await
}

/// Fire-and-forget variant — logs but does not propagate errors.
pub async fn speak_best_effort(text: &str) {
    if let Err(e) = speak(text).await {
        warn!(error = %e, "TTS failed (non-fatal)");
    }
}

fn truncate_for_tts(text: &str) -> String {
    const MAX_CHARS: usize = 500;
    if text.len() <= MAX_CHARS {
        return text.to_owned();
    }
    // Truncate at the last sentence boundary within the limit.
    let slice = &text[..MAX_CHARS];
    if let Some(pos) = slice.rfind(['.', '!', '?']) {
        text[..=pos].to_owned()
    } else {
        slice.to_owned()
    }
}

#[cfg(target_os = "macos")]
async fn run_tts_command(text: &str) -> Result<()> {
    let status = Command::new("say")
        .args(["-r", "160", text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to spawn `say`")?;
    if !status.success() {
        bail!("`say` exited with {:?}", status.code());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn run_tts_command(text: &str) -> Result<()> {
    // Prefer espeak-ng; fall back to espeak.
    let result = Command::new("espeak-ng")
        .args(["-s", "150", text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match result {
        Ok(s) if s.success() => Ok(()),
        _ => {
            let status = Command::new("espeak")
                .args(["-s", "150", text])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .context("failed to spawn `espeak`")?;
            if !status.success() {
                bail!("`espeak` exited with {:?}", status.code());
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
async fn run_tts_command(text: &str) -> Result<()> {
    // Escape single quotes inside the text to prevent PS injection.
    let safe = text.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $s.Rate = 2; \
         $s.Speak('{safe}');"
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to spawn PowerShell TTS")?;
    if !status.success() {
        bail!("PowerShell TTS exited with {:?}", status.code());
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_text_unchanged() {
        let text = "Hello, world.";
        assert_eq!(truncate_for_tts(text), text);
    }

    #[test]
    fn truncate_long_text_at_sentence_boundary() {
        let sentence = "A ".repeat(40); // 80 chars
        let base = sentence.trim_end().to_owned() + ". ";
        let long = base.repeat(10); // > 500 chars
        let result = truncate_for_tts(&long);
        assert!(result.len() <= 500);
        assert!(result.ends_with('.'));
    }

    #[test]
    fn truncate_no_sentence_boundary_falls_back_to_500_chars() {
        let long = "x".repeat(600);
        let result = truncate_for_tts(&long);
        assert_eq!(result.len(), 500);
    }
}
