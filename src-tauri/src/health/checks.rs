//! Installation health checks (design doc §17, Step 2).

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use rusqlite::Connection;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use crate::health::hardware::{self, WhisperModel};
use crate::keychain;
use crate::supabase::resolve_supabase_config;

const OLLAMA_HEALTH_URL: &str = "http://localhost:11434/api/tags";
const OLLAMA_TIMEOUT_SECS: u64 = 2;
const SUPABASE_HEALTH_TIMEOUT_SECS: u64 = 5;
const KEYCHAIN_PROBE_PROVIDER: &str = "health_probe";

/// Individual installation check identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCheck {
    MicrophoneAccess,
    SystemAudioLoopback,
    // Explicit rename because serde snake_case would produce "r_n_noise_preprocessing".
    #[serde(rename = "rnnoise_preprocessing")]
    RNNoisePreprocessing,
    WhisperModel,
    StealthApi,
    PrimaryLlm,
    OllamaAvailability,
    OsKeychain,
    LocalSqlite,
    SupabaseConnection,
    GlobalHotkey,
    PanicHotkey,
}

/// Outcome of a single health check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Result of one health check, with optional fix guidance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HealthCheckResult {
    pub check: HealthCheck,
    pub status: CheckStatus,
    pub message: String,
    pub fix_instruction: Option<String>,
}

/// Run all installation health checks. Re-runnable from onboarding or Settings.
pub async fn run_health_check(
    plugins: &std::collections::HashMap<String, serde_json::Value>,
) -> Vec<HealthCheckResult> {
    let profile = hardware::assess_hardware();
    let supabase_url = resolve_supabase_config(plugins).map(|cfg| cfg.url);

    vec![
        check_microphone_access(),
        check_system_audio_loopback(),
        check_rnnoise_preprocessing(),
        check_whisper_model(profile.recommended_whisper_model),
        check_stealth_api(),
        check_primary_llm(),
        check_ollama_availability().await,
        check_os_keychain(),
        check_local_sqlite(),
        check_supabase_connection(supabase_url.as_deref()).await,
        check_global_hotkey(),
        check_panic_hotkey(),
    ]
}

fn pass(check: HealthCheck, message: impl Into<String>) -> HealthCheckResult {
    HealthCheckResult {
        check,
        status: CheckStatus::Pass,
        message: message.into(),
        fix_instruction: None,
    }
}

fn warn(
    check: HealthCheck,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> HealthCheckResult {
    HealthCheckResult {
        check,
        status: CheckStatus::Warn,
        message: message.into(),
        fix_instruction: Some(fix.into()),
    }
}

fn fail(
    check: HealthCheck,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> HealthCheckResult {
    HealthCheckResult {
        check,
        status: CheckStatus::Fail,
        message: message.into(),
        fix_instruction: Some(fix.into()),
    }
}

fn check_microphone_access() -> HealthCheckResult {
    warn(
        HealthCheck::MicrophoneAccess,
        "Microphone access has not been verified yet.",
        "Before your first live session, grant microphone permission in system Settings → Privacy → Microphone, then retry.",
    )
}

fn check_system_audio_loopback() -> HealthCheckResult {
    #[cfg(target_os = "linux")]
    {
        check_system_audio_loopback_linux()
    }
    #[cfg(target_os = "macos")]
    {
        if blackhole_installed() {
            return pass(
                HealthCheck::SystemAudioLoopback,
                "BlackHole virtual audio driver detected.",
            );
        }
        return warn(
            HealthCheck::SystemAudioLoopback,
            "System audio loopback is not configured.",
            "BlackHole virtual audio driver required. Click here to install: https://existential.audio/blackhole/ — then create a multi-output device in Audio MIDI Setup.",
        );
    }
    #[cfg(target_os = "windows")]
    {
        return pass(
            HealthCheck::SystemAudioLoopback,
            "WASAPI loopback is supported natively.",
        );
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        warn(
            HealthCheck::SystemAudioLoopback,
            "System audio loopback could not be verified on this platform.",
            "Configure platform loopback capture before starting a live session.",
        )
    }
}

#[cfg(target_os = "linux")]
fn check_system_audio_loopback_linux() -> HealthCheckResult {
    if is_x11_session() {
        return warn(
            HealthCheck::SystemAudioLoopback,
            "System audio loopback may work, but stealth mode requires Wayland.",
            "PipeWire is required. Flint captures system audio from your default sink's .monitor source — do NOT run `pactl load-module module-loopback` (that routes your mic to your speakers).",
        );
    }

    if pipewire_available() {
        return pass(
            HealthCheck::SystemAudioLoopback,
            "PipeWire detected — system audio loopback is supported.",
        );
    }

    if pulseaudio_available() {
        return warn(
            HealthCheck::SystemAudioLoopback,
            "PulseAudio detected; PipeWire is recommended for loopback.",
            "Migrate to PipeWire for reliable system-audio capture. Do NOT run `pactl load-module module-loopback` — it routes your mic to your speakers.",
        );
    }

    fail(
        HealthCheck::SystemAudioLoopback,
        "No supported audio server detected for system loopback.",
        "Install PipeWire with loopback support. On Ubuntu: sudo apt install pipewire pipewire-pulse wireplumber.",
    )
}

fn check_rnnoise_preprocessing() -> HealthCheckResult {
    warn(
        HealthCheck::RNNoisePreprocessing,
        "RNNoise preprocessing has not been benchmarked on this device yet.",
        "RNNoise will run automatically when live audio starts. If responses lag, reduce background noise or lower the Whisper model tier.",
    )
}

fn check_whisper_model(recommended: WhisperModel) -> HealthCheckResult {
    let model_file = whisper_model_filename(recommended);
    if whisper_model_exists(&model_file) {
        return pass(
            HealthCheck::WhisperModel,
            format!("Whisper model {model_file} found locally."),
        );
    }

    warn(
        HealthCheck::WhisperModel,
        format!("Whisper model {model_file} is not installed."),
        format!(
            "Download {model_file} into ~/.cache/whisper/ before your first session. Flint will prompt you during session setup if it is still missing."
        ),
    )
}

fn whisper_model_filename(model: WhisperModel) -> String {
    format!("ggml-{}.bin", model.as_str())
}

fn whisper_model_exists(filename: &str) -> bool {
    whisper_search_dirs()
        .into_iter()
        .any(|dir| dir.join(filename).is_file())
}

fn whisper_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".cache/whisper"));
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        dirs.push(PathBuf::from(cache).join("whisper"));
    }
    dirs
}

fn check_stealth_api() -> HealthCheckResult {
    #[cfg(target_os = "linux")]
    {
        if is_x11_session() {
            return fail(
                HealthCheck::StealthApi,
                "Stealth mode requires Wayland. X11 is not supported.",
                "Log out and start a Wayland session (e.g. Ubuntu on Wayland), then re-run the health check.",
            );
        }
        if is_wayland_session() {
            return pass(
                HealthCheck::StealthApi,
                "Wayland session detected — compositor capture exclusion is supported.",
            );
        }
        warn(
            HealthCheck::StealthApi,
            "Could not confirm a Wayland session for stealth mode.",
            "Use a Wayland desktop session. X11 cannot hide the overlay from screen capture.",
        )
    }

    #[cfg(target_os = "windows")]
    {
        return pass(
            HealthCheck::StealthApi,
            "Windows display affinity API is available for capture exclusion.",
        );
    }

    #[cfg(target_os = "macos")]
    {
        return pass(
            HealthCheck::StealthApi,
            "macOS window sharing exclusion (NSWindow.sharingType = .none) is available.",
        );
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        warn(
            HealthCheck::StealthApi,
            "Stealth capture exclusion could not be verified on this platform.",
            "Confirm overlay stealth support before starting a live session.",
        )
    }
}

fn check_primary_llm() -> HealthCheckResult {
    let groq = keychain::get_api_key("groq").is_ok();
    let openai = keychain::get_api_key("openai").is_ok();
    let anthropic = keychain::get_api_key("anthropic").is_ok();

    if groq || openai || anthropic {
        let provider = if groq {
            "Groq"
        } else if openai {
            "OpenAI"
        } else {
            "Anthropic"
        };
        return pass(
            HealthCheck::PrimaryLlm,
            format!("{provider} API key found in the OS keychain."),
        );
    }

    warn(
        HealthCheck::PrimaryLlm,
        "No cloud LLM API key configured.",
        "Add a Groq, OpenAI, or Anthropic API key in Settings → Providers. Ollama can be used as a fallback when running locally.",
    )
}

async fn check_ollama_availability() -> HealthCheckResult {
    let client = match Client::builder()
        .timeout(Duration::from_secs(OLLAMA_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return fail(
                HealthCheck::OllamaAvailability,
                "Could not initialise the Ollama health check.",
                "Ensure Ollama is installed from https://ollama.com and running on localhost:11434.",
            );
        }
    };

    match client.get(OLLAMA_HEALTH_URL).send().await {
        Ok(resp) if resp.status().is_success() => pass(
            HealthCheck::OllamaAvailability,
            "Ollama is running on localhost:11434.",
        ),
        Ok(_) => warn(
            HealthCheck::OllamaAvailability,
            "Ollama responded but returned an unexpected status.",
            "Restart Ollama: ollama serve — then pull a model: ollama pull llama3.2",
        ),
        Err(err) if err.is_timeout() || err.is_connect() => warn(
            HealthCheck::OllamaAvailability,
            "Ollama is not reachable on localhost:11434.",
            "Install and start Ollama (ollama serve). Local fallback and question detection require it on Tier 2+ hardware.",
        ),
        Err(_) => warn(
            HealthCheck::OllamaAvailability,
            "Could not verify Ollama availability.",
            "Install Ollama from https://ollama.com and ensure it is running before your first session.",
        ),
    }
}

fn check_os_keychain() -> HealthCheckResult {
    let probe = SecretString::new("flint-health-probe".into());
    let provider = KEYCHAIN_PROBE_PROVIDER;

    if keychain::store_api_key(provider, probe.clone()).is_err() {
        return keychain_probe_failure_or_warn(
            fail(
                HealthCheck::OsKeychain,
                "Could not write to the OS keychain.",
                "Grant Flint access to the system credential store, then retry. On Linux, unlock your login keyring.",
            ),
        );
    }

    let read_back = match keychain::get_api_key(provider) {
        Ok(value) => value,
        Err(_) => {
            let _ = keychain::delete_api_key(provider);
            return keychain_probe_failure_or_warn(fail(
                HealthCheck::OsKeychain,
                "Could not read from the OS keychain.",
                "Unlock your system keyring and retry. On Linux, ensure Secret Service (GNOME Keyring) is running.",
            ));
        }
    };

    let _ = keychain::delete_api_key(provider);

    if read_back.expose_secret() != "flint-health-probe" {
        return keychain_probe_failure_or_warn(fail(
            HealthCheck::OsKeychain,
            "OS keychain round-trip returned unexpected data.",
            "Retry the health check. If this persists, restart the system credential service.",
        ));
    }

    pass(
        HealthCheck::OsKeychain,
        "OS keychain read/write test passed.",
    )
}

/// When the round-trip probe fails but existing Flint credentials are readable
/// (common on Linux when the login keyring is locked for writes), downgrade to
/// a warning so dev sessions are not blocked.
fn keychain_probe_failure_or_warn(result: HealthCheckResult) -> HealthCheckResult {
    if result.status != CheckStatus::Fail {
        return result;
    }
    if keychain::get_auth_token().is_ok() {
        return warn(
            HealthCheck::OsKeychain,
            "Keychain probe failed, but existing Flint credentials are readable.",
            result.fix_instruction.unwrap_or_else(|| {
                "Unlock your login keyring if you need to store new API keys.".into()
            }),
        );
    }
    result
}

fn check_local_sqlite() -> HealthCheckResult {
    let path = std::env::temp_dir().join(format!("flint_health_check_{}.db", uuid::Uuid::new_v4()));

    let outcome = (|| -> anyhow::Result<()> {
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE health_probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO health_probe (value) VALUES ('ok');",
        )?;
        let value: String =
            conn.query_row("SELECT value FROM health_probe WHERE id = 1", [], |row| {
                row.get(0)
            })?;
        anyhow::ensure!(value == "ok", "unexpected probe value");
        Ok(())
    })();

    let _ = std::fs::remove_file(&path);

    match outcome {
        Ok(()) => pass(
            HealthCheck::LocalSqlite,
            "Local SQLite database created, written, and read successfully (WAL mode).",
        ),
        Err(_) => fail(
            HealthCheck::LocalSqlite,
            "Local SQLite database could not be created or written.",
            "Check disk space and permissions for your Flint app data directory.",
        ),
    }
}

async fn check_supabase_connection(supabase_url: Option<&str>) -> HealthCheckResult {
    let Some(base_url) = supabase_url else {
        return fail(
            HealthCheck::SupabaseConnection,
            "Supabase URL is not configured.",
            "Export FLINT_SUPABASE_URL and FLINT_SUPABASE_ANON_KEY before `npm run tauri dev`, or set plugins.supabase in tauri.conf.json.",
        );
    };

    let health_url = format!("{base_url}/auth/v1/health");
    let client = match Client::builder()
        .timeout(Duration::from_secs(SUPABASE_HEALTH_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return fail(
                HealthCheck::SupabaseConnection,
                "Could not initialise the Supabase health check.",
                "Check your network connection and Supabase project URL, then retry.",
            );
        }
    };

    match client.get(&health_url).send().await {
        Ok(resp) if resp.status().is_success() => pass(
            HealthCheck::SupabaseConnection,
            "Supabase auth service is reachable.",
        ),
        Ok(resp) if resp.status().is_client_error() => fail(
            HealthCheck::SupabaseConnection,
            "Supabase URL responded with an error.",
            "Verify plugins.supabase.url in tauri.conf.json points to your Supabase project.",
        ),
        Ok(_) => warn(
            HealthCheck::SupabaseConnection,
            "Supabase returned an unexpected response.",
            "Confirm your Supabase project is running and the URL is correct, then retry.",
        ),
        Err(err) if err.is_timeout() || err.is_connect() => fail(
            HealthCheck::SupabaseConnection,
            "Flint could not reach the auth service. Check your connection.",
            "Verify internet access and that your Supabase project URL is correct.",
        ),
        Err(_) => fail(
            HealthCheck::SupabaseConnection,
            "Flint could not reach the auth service. Check your connection.",
            "Check your network and Supabase project status, then retry.",
        ),
    }
}

fn check_global_hotkey() -> HealthCheckResult {
    #[cfg(target_os = "linux")]
    if is_wayland_session() {
        return warn(
            HealthCheck::GlobalHotkey,
            "Wayland session — Ctrl+Alt+Space works while Flint is focused; true global capture requires a desktop shortcut or X11.",
            "On GNOME/KDE: Settings → Keyboard → Custom Shortcuts to run a Flint trigger command, or use the in-app Ask button. While testing, click the Flint window first, then press Ctrl+Alt+Space.",
        );
    }
    warn(
        HealthCheck::GlobalHotkey,
        "Global hotkey registration is verified at session start.",
        "Default: Ctrl+Alt+Space to trigger a response. Hotkeys are registered when the app starts — retry if registration fails.",
    )
}

fn check_panic_hotkey() -> HealthCheckResult {
    #[cfg(target_os = "linux")]
    if is_wayland_session() {
        return warn(
            HealthCheck::PanicHotkey,
            "Wayland session — Ctrl+Alt+Shift+Space hides the overlay while Flint is focused.",
            "Bind a desktop shortcut if you need panic hide when another app has focus.",
        );
    }
    warn(
        HealthCheck::PanicHotkey,
        "Panic hotkey registration is verified at session start.",
        "Default: Ctrl+Alt+Shift+Space to hide the overlay.",
    )
}

#[cfg(target_os = "linux")]
fn is_wayland_session() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|s| s.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn is_x11_session() -> bool {
    if is_wayland_session() {
        return false;
    }
    std::env::var("DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|s| s.eq_ignore_ascii_case("x11"))
            .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn pipewire_available() -> bool {
    command_succeeds("pw-cli", &["info", "0"])
        || command_output_contains("pactl", &["info"], "PipeWire")
}

#[cfg(target_os = "linux")]
fn pulseaudio_available() -> bool {
    command_succeeds("pactl", &["info"])
}

#[cfg(target_os = "macos")]
fn blackhole_installed() -> bool {
    std::process::Command::new("system_profiler")
        .args(["SPAudioDataType"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("BlackHole"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn command_output_contains(program: &str, args: &[&str], needle: &str) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(needle))
        .unwrap_or(false)
}

/// Stealth gate before `READY → LIVE`. Hard-fails on X11 (§flint-security).
pub fn run_stealth_self_test() -> Result<(), String> {
    let result = check_stealth_api();
    match result.status {
        CheckStatus::Pass | CheckStatus::Warn => Ok(()),
        CheckStatus::Fail => Err(result.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_model_filename_format() {
        assert_eq!(
            whisper_model_filename(WhisperModel::SmallEn),
            "ggml-small.en.bin"
        );
    }

    #[test]
    fn local_sqlite_probe_succeeds() {
        let result = check_local_sqlite();
        assert_eq!(result.check, HealthCheck::LocalSqlite);
        assert_eq!(result.status, CheckStatus::Pass);
    }

    #[test]
    fn result_status_helpers() {
        let p = pass(HealthCheck::OsKeychain, "ok");
        assert_eq!(p.status, CheckStatus::Pass);
        assert!(p.fix_instruction.is_none());

        let w = warn(HealthCheck::OllamaAvailability, "warn", "fix it");
        assert_eq!(w.status, CheckStatus::Warn);
        assert_eq!(w.fix_instruction.as_deref(), Some("fix it"));
    }
}
