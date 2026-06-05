//! Question bank — typed schema, deserialiser, and loader.
//!
//! Each domain lives in its own JSON file under `evals/questions/`.
//! Question files are versioned with the rest of the repo so reviewers can
//! see prompt changes alongside the test cases they affect.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::EvalError;

/// Coarse-grained domain taxonomy. Matches design doc §20 Question Bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    SoftwareEngineering,
    ProductManagement,
    Finance,
    Marketing,
    Sales,
    Operations,
    Universal,
}

impl Domain {
    pub const ALL: &'static [Domain] = &[
        Domain::SoftwareEngineering,
        Domain::ProductManagement,
        Domain::Finance,
        Domain::Marketing,
        Domain::Sales,
        Domain::Operations,
        Domain::Universal,
    ];

    pub fn file_stem(self) -> &'static str {
        match self {
            Domain::SoftwareEngineering => "software_engineering",
            Domain::ProductManagement => "product_management",
            Domain::Finance => "finance",
            Domain::Marketing => "marketing",
            Domain::Sales => "sales",
            Domain::Operations => "operations",
            Domain::Universal => "universal",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Domain::SoftwareEngineering => "software engineering",
            Domain::ProductManagement => "product management",
            Domain::Finance => "finance",
            Domain::Marketing => "marketing",
            Domain::Sales => "sales",
            Domain::Operations => "operations",
            Domain::Universal => "universal",
        }
    }
}

/// Fine-grained category within a domain (used for reporting only).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Technical,
    Behavioural,
    SystemDesign,
    Strategy,
    Prioritisation,
    Case,
    Campaign,
    ObjectionHandling,
    Process,
    ProblemSolving,
    StarStory,
    Strengths,
    Weaknesses,
    Introduction,
}

/// Single test case. `context` is optional — when set, it is injected as
/// `[data]` into the prompt to exercise the RAG path; otherwise the judge
/// scores groundedness against an empty context.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Question {
    pub id: String,
    pub domain: Domain,
    pub category: Category,
    pub text: String,
    #[serde(default)]
    pub context: Vec<String>,
    /// Optional reference answer used by the LLM judge as a north-star.
    #[serde(default)]
    pub reference_answer: Option<String>,
}

/// Top-level container deserialised from each `<domain>.json` file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuestionFile {
    pub domain: Domain,
    pub version: u32,
    pub questions: Vec<Question>,
}

/// Whole bank indexed by domain. Built once at the start of an eval run.
#[derive(Debug, Default)]
pub struct QuestionBank {
    by_domain: BTreeMap<Domain, Vec<Question>>,
}

impl QuestionBank {
    pub fn load(questions_dir: &Path) -> Result<Self, EvalError> {
        let mut bank = QuestionBank::default();
        for domain in Domain::ALL {
            let path = questions_dir.join(format!("{}.json", domain.file_stem()));
            if !path.exists() {
                tracing::warn!(
                    path = %path.display(),
                    domain = ?domain,
                    "question file missing — domain will be empty"
                );
                continue;
            }
            let raw = fs::read_to_string(&path).map_err(|e| EvalError::BaselineRead {
                path: path.display().to_string(),
                source: e,
            })?;
            let parsed: QuestionFile =
                serde_json::from_str(&raw).map_err(|e| EvalError::BankParse {
                    path: path.display().to_string(),
                    source: e,
                })?;
            if parsed.domain != *domain {
                tracing::warn!(
                    file = %path.display(),
                    declared = ?parsed.domain,
                    expected = ?domain,
                    "domain mismatch — using file contents anyway"
                );
            }
            bank.by_domain.insert(*domain, parsed.questions);
        }
        Ok(bank)
    }

    pub fn questions(&self) -> impl Iterator<Item = &Question> {
        self.by_domain.values().flatten()
    }

    pub fn by_domain(&self, domain: Domain) -> &[Question] {
        self.by_domain
            .get(&domain)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn total(&self) -> usize {
        self.by_domain.values().map(Vec::len).sum()
    }

    pub fn domains(&self) -> impl Iterator<Item = Domain> + '_ {
        self.by_domain.keys().copied()
    }
}

/// Filter helper used by the CLI when running a subset of the bank.
pub fn select_questions(
    bank: &QuestionBank,
    domain: Option<Domain>,
    limit: Option<usize>,
) -> Vec<Question> {
    let mut selected: Vec<Question> = match domain {
        Some(d) => bank.by_domain(d).to_vec(),
        None => bank.questions().cloned().collect(),
    };
    if let Some(n) = limit {
        selected.truncate(n);
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn write_domain_file(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(format!("{name}.json"));
        fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn loads_empty_bank_when_no_files_present() {
        let dir = tempdir().unwrap();
        let bank = QuestionBank::load(dir.path()).expect("load");
        assert_eq!(bank.total(), 0);
    }

    #[test]
    fn loads_questions_grouped_by_domain() {
        let dir = tempdir().unwrap();
        write_domain_file(
            dir.path(),
            "software_engineering",
            r#"{
                "domain": "software_engineering",
                "version": 1,
                "questions": [
                    {
                        "id": "swe-001",
                        "domain": "software_engineering",
                        "category": "technical",
                        "text": "Explain ownership in Rust."
                    }
                ]
            }"#,
        );
        let bank = QuestionBank::load(dir.path()).expect("load");
        assert_eq!(bank.total(), 1);
        assert_eq!(bank.by_domain(Domain::SoftwareEngineering).len(), 1);
    }

    #[test]
    fn real_question_bank_loads_with_two_hundred_questions() {
        let workspace_root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let questions_dir = std::path::Path::new(&workspace_root).join("questions");
        if !questions_dir.exists() {
            // Lets unit tests pass in stripped-down checkouts.
            return;
        }
        let bank = QuestionBank::load(&questions_dir).expect("load real bank");
        assert_eq!(
            bank.total(),
            200,
            "design doc §20 mandates 200 questions in the v1 bank"
        );
        // Every advertised domain must have at least one question.
        for domain in Domain::ALL {
            assert!(
                !bank.by_domain(*domain).is_empty(),
                "domain {:?} is empty",
                domain
            );
        }
    }

    #[test]
    fn select_respects_domain_and_limit_filters() {
        let dir = tempdir().unwrap();
        write_domain_file(
            dir.path(),
            "universal",
            r#"{
                "domain": "universal",
                "version": 1,
                "questions": [
                    { "id": "u-1", "domain": "universal", "category": "introduction", "text": "Q1" },
                    { "id": "u-2", "domain": "universal", "category": "introduction", "text": "Q2" }
                ]
            }"#,
        );
        let bank = QuestionBank::load(dir.path()).expect("load");
        let one = select_questions(&bank, Some(Domain::Universal), Some(1));
        assert_eq!(one.len(), 1);
        let all = select_questions(&bank, None, None);
        assert_eq!(all.len(), 2);
    }
}
