//! Baseline storage — the most recent passing run is committed to disk
//! and used as the win-rate reference for subsequent runs.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::EvalError;
use crate::report::Report;

const BASELINE_FILENAME: &str = "baseline.json";

pub struct BaselineStore {
    root: PathBuf,
}

impl BaselineStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self) -> PathBuf {
        self.root.join(BASELINE_FILENAME)
    }

    pub fn load(&self) -> Result<Option<Report>, EvalError> {
        let path = self.path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path).map_err(|e| EvalError::BaselineRead {
            path: path.display().to_string(),
            source: e,
        })?;
        let report: Report = serde_json::from_str(&raw)?;
        Ok(Some(report))
    }

    pub fn save(&self, report: &Report) -> Result<(), EvalError> {
        fs::create_dir_all(&self.root)?;
        let json = serde_json::to_string_pretty(report)?;
        fs::write(self.path(), json)?;
        Ok(())
    }

    pub fn archive(&self, report: &Report, archive_dir: &Path) -> Result<PathBuf, EvalError> {
        fs::create_dir_all(archive_dir)?;
        let short_id: String = report.run_id.chars().take(8).collect();
        let filename = format!(
            "{}__{}.json",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
            short_id
        );
        let path = archive_dir.join(filename);
        let json = serde_json::to_string_pretty(report)?;
        fs::write(&path, json)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn fake_report() -> Report {
        Report {
            run_id: "run".into(),
            prompt_version: "v1".into(),
            variants: BTreeMap::new(),
        }
    }

    #[test]
    fn returns_none_when_no_baseline_exists() {
        let dir = tempdir().unwrap();
        let store = BaselineStore::new(dir.path());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn round_trip_save_then_load() {
        let dir = tempdir().unwrap();
        let store = BaselineStore::new(dir.path());
        let report = fake_report();
        store.save(&report).unwrap();
        let loaded = store.load().unwrap().expect("baseline present");
        assert_eq!(loaded.run_id, "run");
    }

    #[test]
    fn archive_writes_timestamped_file() {
        let dir = tempdir().unwrap();
        let store = BaselineStore::new(dir.path());
        let report = fake_report();
        let archive_dir = dir.path().join("archive");
        let path = store.archive(&report, &archive_dir).unwrap();
        assert!(path.exists());
    }
}
