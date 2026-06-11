//! Hardware tier assessment (design doc §17, Step 1).

use sysinfo::System;
use tracing::info;

/// Device performance tier (1 = lowest, 4 = highest).
pub type HardwareTier = u8;

/// Recommended Whisper.cpp model for the detected tier.
///
/// Variants are ordered from smallest/fastest to largest/most accurate so
/// the tier mapping can rely on natural ordering when bumped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum WhisperModel {
    TinyEn,
    BaseEn,
    SmallEn,
    MediumEn,
}

impl WhisperModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TinyEn => "tiny.en",
            Self::BaseEn => "base.en",
            Self::SmallEn => "small.en",
            Self::MediumEn => "medium.en",
        }
    }

    /// Resolve a model name (`tiny.en`, `base.en`, ...) to a typed variant.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "tiny.en" | "tiny" => Some(Self::TinyEn),
            "base.en" | "base" => Some(Self::BaseEn),
            "small.en" | "small" => Some(Self::SmallEn),
            "medium.en" | "medium" => Some(Self::MediumEn),
            _ => None,
        }
    }
}

impl std::fmt::Display for WhisperModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Recommended local/cloud LLM routing for the detected tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LLMConfig {
    pub directional: String,
    pub depth: String,
    pub fallback: Option<String>,
    /// When true, cloud providers are recommended over local Ollama.
    pub cloud_recommended: bool,
}

/// Full hardware profile shown on the health-check screen.
#[derive(Clone, Debug, PartialEq)]
pub struct HardwareProfile {
    pub tier: HardwareTier,
    pub cpu_cores: usize,
    pub ram_gb: f64,
    pub has_gpu: bool,
    pub gpu_vram_gb: Option<f64>,
    pub os: String,
    pub recommended_whisper_model: WhisperModel,
    pub recommended_llm_config: LLMConfig,
}

struct GpuInfo {
    has_any: bool,
    has_dedicated: bool,
    vram_gb: Option<f64>,
}

/// Classify hardware tier from RAM and any-GPU presence (§17 table).
///
/// Used by unit tests and callers that only know whether a GPU is present, not whether it is discrete.
/// For full assessment including dedicated-GPU tier-4 promotion, see [`calculate_tier_detailed`].
#[allow(dead_code)] // used by health checks UI and unit tests
pub fn calculate_tier(ram_gb: f64, has_gpu: bool) -> HardwareTier {
    if ram_gb >= 32.0 {
        4
    } else if ram_gb >= 16.0 || has_gpu {
        3
    } else if ram_gb >= 8.0 {
        2
    } else {
        1
    }
}

/// Classify tier using detected any-GPU and dedicated-GPU signals.
pub fn calculate_tier_detailed(
    ram_gb: f64,
    has_any_gpu: bool,
    has_dedicated_gpu: bool,
) -> HardwareTier {
    if ram_gb >= 32.0 || has_dedicated_gpu {
        4
    } else if ram_gb >= 16.0 || has_any_gpu {
        3
    } else if ram_gb >= 8.0 {
        2
    } else {
        1
    }
}

/// Detect hardware, classify tier, and log the result at startup.
pub fn assess_hardware() -> HardwareProfile {
    let mut system = System::new_all();
    system.refresh_all();

    let cpu_cores = system.cpus().len().max(1);
    let ram_gb = bytes_to_gb(system.total_memory());
    let os = format_os();
    let gpu = detect_gpu();

    let tier = calculate_tier_detailed(ram_gb, gpu.has_any, gpu.has_dedicated);
    let recommended_whisper_model = recommended_whisper_model(tier);
    let recommended_llm_config = recommended_llm_config(tier);

    info!(
        tier = tier,
        ram_gb = ram_gb,
        cpu_cores = cpu_cores,
        has_gpu = gpu.has_any,
        "hardware assessment complete"
    );

    HardwareProfile {
        tier,
        cpu_cores,
        ram_gb,
        has_gpu: gpu.has_any,
        gpu_vram_gb: gpu.vram_gb,
        os,
        recommended_whisper_model,
        recommended_llm_config,
    }
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

fn format_os() -> String {
    let name = System::name().unwrap_or_else(|| "Unknown".to_string());
    let version = System::os_version().unwrap_or_default();
    if version.is_empty() {
        name
    } else {
        format!("{name} {version}")
    }
}

fn recommended_whisper_model(tier: HardwareTier) -> WhisperModel {
    match tier {
        1 => WhisperModel::TinyEn,
        2 => WhisperModel::BaseEn,
        3 => WhisperModel::SmallEn,
        _ => WhisperModel::SmallEn,
    }
}

fn recommended_llm_config(tier: HardwareTier) -> LLMConfig {
    match tier {
        1 => LLMConfig {
            directional: "Groq Llama 3.3 70B (cloud)".into(),
            depth: "Groq Llama 3.3 70B (cloud)".into(),
            fallback: None,
            cloud_recommended: true,
        },
        2 => LLMConfig {
            directional: "Ollama Llama 3.2 3B (local)".into(),
            depth: "Groq Llama 3.3 70B (cloud)".into(),
            fallback: Some("Ollama Llama 3.2 3B (local)".into()),
            cloud_recommended: false,
        },
        3 => LLMConfig {
            directional: "Ollama Llama 3.2 3B (local)".into(),
            depth: "Ollama Llama 3.1 8B (local)".into(),
            fallback: Some("Groq Llama 3.3 70B (cloud)".into()),
            cloud_recommended: false,
        },
        _ => LLMConfig {
            directional: "Groq Llama 3.3 70B (cloud)".into(),
            depth: "Anthropic Claude 3.5 Sonnet (cloud)".into(),
            fallback: Some("Ollama Llama 3.1 8B (local)".into()),
            cloud_recommended: false,
        },
    }
}

fn detect_gpu() -> GpuInfo {
    #[cfg(target_os = "linux")]
    {
        detect_gpu_linux()
    }
    #[cfg(target_os = "macos")]
    {
        detect_gpu_macos()
    }
    #[cfg(target_os = "windows")]
    {
        detect_gpu_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        GpuInfo {
            has_any: false,
            has_dedicated: false,
            vram_gb: None,
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_gpu_linux() -> GpuInfo {
    let mut has_any = false;
    let mut has_dedicated = false;
    let mut max_vram_gb: Option<f64> = None;

    let drm = std::path::Path::new("/sys/class/drm");
    let Ok(entries) = std::fs::read_dir(drm) else {
        return GpuInfo {
            has_any: false,
            has_dedicated: false,
            vram_gb: None,
        };
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }

        let device = entry.path().join("device");
        let vendor = read_hex_u32(&device.join("vendor"));
        let class = read_hex_u32(&device.join("class"));

        // Skip display controllers that are not GPUs (e.g. audio).
        if class.map(|c| (c >> 16) != 0x03).unwrap_or(false) {
            continue;
        }

        let (any, dedicated) = classify_pci_vendor(vendor);
        if any {
            has_any = true;
        }
        if dedicated {
            has_dedicated = true;
        }

        if let Some(vram_bytes) = read_vram_bytes(&device) {
            let vram_gb = bytes_to_gb(vram_bytes);
            max_vram_gb = Some(max_vram_gb.map_or(vram_gb, |m| m.max(vram_gb)));
        }
    }

    GpuInfo {
        has_any,
        has_dedicated,
        vram_gb: max_vram_gb,
    }
}

#[cfg(target_os = "macos")]
fn detect_gpu_macos() -> GpuInfo {
    // Apple Silicon and discrete Mac GPUs: treat as dedicated-capable for tier 4.
    let has_gpu = std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType"])
        .output()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains("Chipset Model:") && !stdout.contains("Display: None")
        })
        .unwrap_or(false);

    GpuInfo {
        has_any: has_gpu,
        has_dedicated: has_gpu,
        vram_gb: None,
    }
}

#[cfg(target_os = "windows")]
fn detect_gpu_windows() -> GpuInfo {
    // DXGI enumeration is out of scope for sysinfo-only v1; RAM tiers still apply.
    GpuInfo {
        has_any: false,
        has_dedicated: false,
        vram_gb: None,
    }
}

#[cfg(target_os = "linux")]
fn read_hex_u32(path: &std::path::Path) -> Option<u32> {
    let raw = std::fs::read_to_string(path).ok()?;
    u32::from_str_radix(raw.trim().trim_start_matches("0x"), 16).ok()
}

#[cfg(target_os = "linux")]
fn read_vram_bytes(device: &std::path::Path) -> Option<u64> {
    for name in ["mem_info_vram_total", "mem_info_vram_used"] {
        if let Ok(raw) = std::fs::read_to_string(device.join(name)) {
            if let Ok(bytes) = raw.trim().parse::<u64>() {
                if bytes > 0 {
                    return Some(bytes);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn classify_pci_vendor(vendor: Option<u32>) -> (bool, bool) {
    match vendor {
        Some(0x10de) => (true, true),                                 // NVIDIA
        Some(0x1002) | Some(0x1022) => (true, true),                  // AMD
        Some(0x8086) => (true, false),                                // Intel integrated
        Some(0x1414) | Some(0x1af4) | Some(0x1b36) => (false, false), // virtual/basic
        Some(_) => (true, false),
        None => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_tier_boundaries() {
        assert_eq!(calculate_tier(6.0, false), 1);
        assert_eq!(calculate_tier(8.0, false), 2);
        assert_eq!(calculate_tier(16.0, false), 3);
        assert_eq!(calculate_tier(32.0, true), 4);
    }

    #[test]
    fn tier_three_from_gpu_without_ram() {
        assert_eq!(calculate_tier(8.0, true), 3);
    }

    #[test]
    fn tier_four_from_dedicated_gpu() {
        assert_eq!(calculate_tier_detailed(16.0, true, true), 4);
        assert_eq!(calculate_tier_detailed(16.0, true, false), 3);
    }

    #[test]
    fn whisper_model_by_tier_is_monotonic() {
        assert_eq!(recommended_whisper_model(1), WhisperModel::TinyEn);
        assert_eq!(recommended_whisper_model(2), WhisperModel::BaseEn);
        assert_eq!(recommended_whisper_model(3), WhisperModel::SmallEn);
        assert_eq!(recommended_whisper_model(4), WhisperModel::SmallEn);
    }

    #[test]
    fn whisper_model_from_name_round_trip() {
        for m in [
            WhisperModel::TinyEn,
            WhisperModel::BaseEn,
            WhisperModel::SmallEn,
            WhisperModel::MediumEn,
        ] {
            assert_eq!(WhisperModel::from_name(m.as_str()), Some(m));
        }
        assert_eq!(WhisperModel::from_name("Small"), Some(WhisperModel::SmallEn));
        assert!(WhisperModel::from_name("bogus").is_none());
    }
}
