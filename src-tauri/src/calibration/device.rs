//! Stable device fingerprint for per-device mic calibration preferences.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};

/// Hash of OS + default mic name + default output name.
pub fn device_fingerprint() -> Result<String> {
    let host = cpal::default_host();
    let os = std::env::consts::OS.to_string();
    let mic_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_else(|| "unknown-mic".to_string());
    let output_name = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_else(|| "unknown-output".to_string());

    let mut hasher = DefaultHasher::new();
    os.hash(&mut hasher);
    mic_name.hash(&mut hasher);
    output_name.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

pub fn device_fingerprint_or_fallback() -> String {
    device_fingerprint().unwrap_or_else(|_| "unknown-device".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_stable_within_process() {
        let a = device_fingerprint_or_fallback();
        let b = device_fingerprint_or_fallback();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }
}
