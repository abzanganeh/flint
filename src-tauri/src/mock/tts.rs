//! Platform TTS abstraction.
//!
//! macOS   → `say -r 160 "<text>"`
//! Linux   → Piper (neural, local) → OpenAI TTS (neural, cloud) → espeak-ng → espeak
//! Windows → PowerShell `Add-Type -A System.Speech; …SpeechSynthesizer`
//!
//! On Linux the backend is probed in priority order on every call; the first
//! one that succeeds is used.  All failures fall through silently so the
//! interviewer keeps working even without any TTS engine installed.
//!
//! ## Piper setup (one-time)
//! ```
//! pip install piper-tts
//! mkdir -p ~/.local/share/piper
//! cd ~/.local/share/piper
//! wget https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx
//! wget https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json
//! ```
//!
//! ## OpenAI TTS setup
//! Store an OpenAI API key via the Flint settings panel — no extra config needed.

use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{instrument, warn};

#[cfg(target_os = "linux")]
use {
    crate::keychain,
    secrecy::ExposeSecret,
    std::path::{Path, PathBuf},
    tokio::io::AsyncWriteExt,
};

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

// ── Linux TTS backend chain ───────────────────────────────────────────────────

#[cfg(target_os = "linux")]
async fn run_tts_command(text: &str) -> Result<()> {
    // 1. Piper — local neural TTS (sounds natural, no internet required).
    if let Some(piper_bin) = find_in_path("piper") {
        if let Some(model) = find_piper_model() {
            match run_piper(text, &piper_bin, &model).await {
                Ok(()) => return Ok(()),
                Err(e) => warn!(error = %e, "piper TTS failed, trying next backend"),
            }
        }
    }

    // 2. OpenAI TTS — cloud neural TTS (requires openai key in keychain).
    if let Ok(key) = keychain::get_api_key("openai") {
        match run_openai_tts(text, key.expose_secret()).await {
            Ok(()) => return Ok(()),
            Err(e) => warn!(error = %e, "OpenAI TTS failed, falling back to espeak-ng"),
        }
    }

    // 3. espeak-ng / espeak — phonetic fallback (always works, robotic quality).
    run_espeak(text).await
}

/// Pipe `text` into the piper binary, write a WAV to a temp file, play with aplay.
#[cfg(target_os = "linux")]
async fn run_piper(text: &str, piper_bin: &Path, model: &Path) -> Result<()> {
    let wav_path = std::env::temp_dir().join("flint_tts.wav");

    let mut child = Command::new(piper_bin)
        .args([
            "--model",
            model.to_str().context("non-UTF-8 model path")?,
            "--output_file",
            wav_path.to_str().context("non-UTF-8 temp path")?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn piper")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .await
            .context("write text to piper stdin")?;
        // Dropping stdin closes the pipe and signals EOF to piper.
    }

    let status = child.wait().await.context("piper wait")?;
    if !status.success() {
        bail!("piper exited with {:?}", status.code());
    }

    // Play the generated WAV — aplay reads the WAV header for sample rate automatically.
    let play = Command::new("aplay")
        .arg(&wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to spawn aplay")?;
    if !play.success() {
        bail!("aplay exited with {:?}", play.code());
    }
    Ok(())
}

/// Call the OpenAI TTS API, save the MP3 to a temp file, and play it.
#[cfg(target_os = "linux")]
async fn run_openai_tts(text: &str, api_key: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&serde_json::json!({
            "model": "tts-1",
            "voice": "onyx",
            "input": text,
        }))
        .send()
        .await
        .context("OpenAI TTS request")?;

    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("OpenAI TTS HTTP {code}: {body}");
    }

    let mp3_bytes = resp
        .bytes()
        .await
        .context("read OpenAI TTS response body")?;
    let mp3_path = std::env::temp_dir().join("flint_tts.mp3");
    tokio::fs::write(&mp3_path, &mp3_bytes)
        .await
        .context("write TTS mp3 to temp file")?;

    // Play the MP3 — try mpv first, then ffplay (both common on Linux desktops).
    for player in &["mpv", "ffplay"] {
        if find_in_path(player).is_none() {
            continue;
        }
        let args: &[&str] = match *player {
            "mpv" => &["--no-video", "--no-terminal", "--really-quiet"],
            "ffplay" => &["-nodisp", "-autoexit", "-loglevel", "quiet"],
            _ => &[],
        };
        let status = Command::new(player)
            .args(args)
            .arg(&mp3_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .with_context(|| format!("failed to spawn {player}"))?;
        if status.success() {
            return Ok(());
        }
        warn!(player, "MP3 player exited non-zero, trying next");
    }
    bail!("no working MP3 player found (tried mpv, ffplay)");
}

/// espeak-ng / espeak phonetic fallback.
#[cfg(target_os = "linux")]
async fn run_espeak(text: &str) -> Result<()> {
    let result = Command::new("espeak-ng")
        .args(["-s", "150", text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match result {
        Ok(s) if s.success() => return Ok(()),
        _ => {}
    }

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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Find `name` in the current `PATH` without spawning a subprocess.
#[cfg(target_os = "linux")]
fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path_var| {
        std::env::split_paths(&path_var)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// Look for any `.onnx` model file in standard Piper model directories.
///
/// Search order:
///   1. `~/.local/share/piper/`   — user install (recommended)
///   2. `/usr/local/share/piper/` — system install via package manager
///   3. `/usr/share/piper/`       — distro package
#[cfg(target_os = "linux")]
fn find_piper_model() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let search_dirs = [
        home.join(".local/share/piper"),
        PathBuf::from("/usr/local/share/piper"),
        PathBuf::from("/usr/share/piper"),
    ];
    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let model = entries
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().and_then(|e| e.to_str()) == Some("onnx"));
            if model.is_some() {
                return model;
            }
        }
    }
    None
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
