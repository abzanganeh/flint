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
//! ```text
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
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{instrument, warn};

#[cfg(target_os = "linux")]
use {
    crate::keychain,
    secrecy::ExposeSecret,
    std::path::{Path, PathBuf},
    tokio::io::AsyncWriteExt,
};

/// Kill any in-flight TTS subprocess (espeak, aplay, mpv, etc.).
pub async fn stop_active() {
    let mut guard = active_pids().lock().await;
    for pid in guard.drain(..) {
        let _ = Command::new("kill")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

fn active_pids() -> &'static Mutex<Vec<u32>> {
    static PIDS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    PIDS.get_or_init(|| Mutex::new(Vec::new()))
}

async fn track_pid(pid: u32) {
    active_pids().lock().await.push(pid);
}

async fn untrack_pid(pid: u32) {
    let mut guard = active_pids().lock().await;
    guard.retain(|p| *p != pid);
}

async fn run_tracked(mut child: tokio::process::Child) -> Result<std::process::ExitStatus> {
    let pid = child.id();
    if let Some(pid) = pid {
        track_pid(pid).await;
    }
    let status = child.wait().await.context("wait on TTS child")?;
    if let Some(pid) = pid {
        untrack_pid(pid).await;
    }
    Ok(status)
}

/// Speak `text` using the platform TTS engine and wait for it to finish.
///
/// Silently truncates to 500 characters to avoid runaway TTS on malformed
/// LLM output.
#[instrument(skip(text), fields(len = text.len()))]
pub async fn speak(text: &str) -> Result<()> {
    let text = normalize_tts_pronunciation(text);
    let text = truncate_for_tts(&text);
    run_tts_command(&text).await
}

/// Fire-and-forget variant — logs but does not propagate errors.
pub async fn speak_best_effort(text: &str) {
    if let Err(e) = speak(text).await {
        warn!(error = %e, "TTS failed (non-fatal)");
    }
}

/// Fix words that TTS engines commonly mispronounce in interview prompts.
fn normalize_tts_pronunciation(text: &str) -> String {
    // "resume" (CV) → re-ZOOM; "résumé" → reh-zoo-MAY on most engines.
    let text = replace_word(text, "resumes", "résumés");
    let text = replace_word(&text, "Resumes", "Résumés");
    let text = replace_word(&text, "resume", "résumé");
    let text = replace_word(&text, "Resume", "Résumé");
    replace_word(&text, "resumé", "résumé")
}

/// Replace `from` with `to` only on whole-word matches (ASCII alphanumeric boundaries).
fn replace_word(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find(from) {
        out.push_str(&rest[..idx]);
        let after = idx + from.len();
        let word_start = idx == 0
            || !rest[..idx]
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let word_end = after >= rest.len()
            || !rest[after..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if word_start && word_end {
            out.push_str(to);
        } else {
            out.push_str(from);
        }
        rest = &rest[after..];
    }
    out.push_str(rest);
    out
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
    let child = Command::new("say")
        .args(["-r", "160", text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `say`")?;
    let status = run_tracked(child).await.context("wait on `say`")?;
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

/// Pipe `text` into piper, write WAV to a temp file, play with aplay.
///
/// stderr is captured and logged at WARN so piper failures are visible
/// in `tauri dev` output — previously they were silenced and the robotic
/// espeak-ng fallback gave no indication of why.
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
        .stderr(Stdio::piped()) // capture — not null — so we can log failures
        .spawn()
        .context("failed to spawn piper")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .await
            .context("write text to piper stdin")?;
        // Dropping stdin closes the pipe, signals EOF to piper.
    }

    let out = child.wait_with_output().await.context("piper wait")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("piper failed (stderr: {})", stderr.trim());
    }

    let play = Command::new("aplay")
        .arg(&wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn aplay")?;
    let play_status = run_tracked(play).await.context("aplay wait")?;
    if !play_status.success() {
        bail!("aplay exited with {:?}", play_status.code());
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
        let status = run_tracked(
            Command::new(player)
                .args(args)
                .arg(&mp3_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .with_context(|| format!("failed to spawn {player}"))?,
        )
        .await
        .with_context(|| format!("wait on {player}"))?;
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
    let result = {
        let child = Command::new("espeak-ng")
            .args(["-s", "150", text])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn espeak-ng")?;
        run_tracked(child).await
    };

    match result {
        Ok(s) if s.success() => return Ok(()),
        _ => {}
    }

    let child = Command::new("espeak")
        .args(["-s", "150", text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `espeak`")?;
    let status = run_tracked(child).await?;
    if !status.success() {
        bail!("`espeak` exited with {:?}", status.code());
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Find a binary by name — checks PATH first, then common Python/conda locations
/// where `piper` is typically installed via `pip install piper-tts`.
///
/// Tauri subprocesses may not inherit a full conda-activated PATH, so we
/// fall back to well-known locations before giving up.
#[cfg(target_os = "linux")]
fn find_in_path(name: &str) -> Option<PathBuf> {
    // 1. Standard PATH search.
    if let Some(p) = std::env::var_os("PATH").and_then(|path_var| {
        std::env::split_paths(&path_var)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    }) {
        return Some(p);
    }

    // 2. Fallback: common locations where piper-tts installs its script.
    //    Ordered by likelihood on a typical dev machine.
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let candidates = [
        // conda base / named env
        home.join("anaconda3/bin").join(name),
        home.join("miniconda3/bin").join(name),
        home.join("miniforge3/bin").join(name),
        home.join(".conda/envs/base/bin").join(name),
        // pip --user install
        home.join(".local/bin").join(name),
        // system Python pip install
        PathBuf::from("/usr/local/bin").join(name),
        PathBuf::from("/usr/bin").join(name),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Find `piper` binary — checks PATH then common conda/pip install locations.
/// Exposed for use by the calibration module.
#[cfg(target_os = "linux")]
pub fn find_piper_bin() -> Option<PathBuf> {
    find_in_path("piper")
}

/// Look for any `.onnx` model file in standard Piper model directories.
/// Exposed for use by the calibration module.
#[cfg(target_os = "linux")]
pub fn find_piper_model_path() -> Option<PathBuf> {
    find_piper_model()
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
    let child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn PowerShell TTS")?;
    let status = run_tracked(child).await.context("wait on PowerShell TTS")?;
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

    #[test]
    fn normalize_resume_cv_pronunciation() {
        assert_eq!(
            normalize_tts_pronunciation("Walk me through your resume."),
            "Walk me through your résumé."
        );
        assert_eq!(
            normalize_tts_pronunciation("Resume mentions device trust"),
            "Résumé mentions device trust"
        );
        assert_eq!(
            normalize_tts_pronunciation("Compare two resumes side by side"),
            "Compare two résumés side by side"
        );
    }

    #[test]
    fn normalize_does_not_touch_presume() {
        assert_eq!(
            normalize_tts_pronunciation("I presume you reviewed the resume."),
            "I presume you reviewed the résumé."
        );
    }
}
