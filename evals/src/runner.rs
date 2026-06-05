//! Eval runner — exercises every (question × variant) pair through the
//! same prompt-building and LLM streaming paths the live orchestrator uses.
//!
//! The runner deliberately does NOT depend on Tauri or the audio pipeline;
//! it loads each prompt variant via `flint_lib::orchestrator::load_prompt`,
//! substitutes template fields, streams tokens from the configured
//! provider, and times TTFT + full stream.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use flint_lib::llm::provider::{CompletionConfig, LLMProvider};
use flint_lib::orchestrator::load_prompt;

use crate::bank::{Domain, Question};
use crate::error::EvalError;
use crate::judge::{Judge, JudgeRequest, JudgeScores};
use crate::metrics::{
    score_conciseness, score_latency, score_structure, ConcisenessOutcome, LatencyOutcome,
    StructureOutcome,
};

/// Prompt variant identifier — matches the filename under
/// `prompts/<category>/<variant>.txt`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PromptVariant {
    Gpt,
    Claude,
    Llama,
    Default,
}

impl PromptVariant {
    pub const PRODUCTION_VARIANTS: &'static [PromptVariant] = &[
        PromptVariant::Gpt,
        PromptVariant::Claude,
        PromptVariant::Llama,
    ];

    pub fn filename(self) -> &'static str {
        match self {
            PromptVariant::Gpt => "gpt",
            PromptVariant::Claude => "claude",
            PromptVariant::Llama => "llama",
            PromptVariant::Default => "default",
        }
    }
}

/// Per-thread scores from a single eval run. The harness scores directional
/// and depth separately because they have different quality criteria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadScore {
    pub response_text: String,
    pub latency: LatencyOutcome,
    pub judge: Option<JudgeScores>,
}

/// One row in the eval report — covers a single (question × variant) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRow {
    pub question_id: String,
    pub domain: Domain,
    pub variant: PromptVariant,
    pub directional: ThreadScore,
    pub depth: ThreadScore,
    pub conciseness: ConcisenessOutcome,
    pub structure: StructureOutcome,
    pub error: Option<String>,
}

/// Top-level container persisted to `evals/results/<timestamp>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    pub run_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub prompt_version: String,
    pub rows: Vec<EvalRow>,
}

/// Configuration passed once when constructing the runner.
pub struct EvalRunnerConfig {
    pub prompts_dir: PathBuf,
    pub provider: Arc<dyn LLMProvider>,
    pub judge: Arc<dyn Judge>,
    pub variants: Vec<PromptVariant>,
    pub max_concurrent: usize,
}

pub struct EvalRunner {
    cfg: EvalRunnerConfig,
}

impl EvalRunner {
    pub fn new(cfg: EvalRunnerConfig) -> Self {
        Self { cfg }
    }

    pub async fn run(&self, questions: &[Question]) -> Result<EvalRun, EvalError> {
        let mut rows: Vec<EvalRow> = Vec::with_capacity(questions.len() * self.cfg.variants.len());

        for variant in &self.cfg.variants {
            for question in questions {
                let row = self
                    .run_one(question, *variant)
                    .await
                    .unwrap_or_else(|e| Self::row_for_error(question, *variant, e));
                rows.push(row);
            }
        }

        Ok(EvalRun {
            run_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now(),
            prompt_version: prompt_version_marker(&self.cfg.prompts_dir),
            rows,
        })
    }

    async fn run_one(
        &self,
        question: &Question,
        variant: PromptVariant,
    ) -> Result<EvalRow, EvalError> {
        let directional =
            self.run_thread("directional", question, variant, 200).await?;
        let depth = self.run_thread("depth", question, variant, 400).await?;

        let conciseness = score_conciseness(&directional.response_text);
        let structure = score_structure(&depth.response_text);

        Ok(EvalRow {
            question_id: question.id.clone(),
            domain: question.domain,
            variant,
            directional,
            depth,
            conciseness,
            structure,
            error: None,
        })
    }

    async fn run_thread(
        &self,
        category: &str,
        question: &Question,
        variant: PromptVariant,
        max_tokens: usize,
    ) -> Result<ThreadScore, EvalError> {
        let template = load_prompt(category, variant.filename(), &self.cfg.prompts_dir)
            .map_err(|e| EvalError::Runner(format!("load {category} prompt failed: {e}")))?;
        let prompt = render_prompt(&template, question);

        let cfg = CompletionConfig {
            temperature: 0.0,
            max_tokens: Some(max_tokens),
            stream: true,
        };
        let started = Instant::now();
        let mut stream = self
            .cfg
            .provider
            .complete_stream(prompt, cfg)
            .await
            .map_err(|e| EvalError::Runner(format!("{category} stream failed: {e}")))?;

        let mut response = String::new();
        let mut ttft_ms = None;
        while let Some(chunk) = stream.next().await {
            let token = chunk.map_err(|e| EvalError::Runner(format!("{category} token: {e}")))?;
            if ttft_ms.is_none() {
                ttft_ms = Some(started.elapsed().as_millis() as u64);
            }
            response.push_str(&token);
        }
        let stream_ms = started.elapsed().as_millis() as u64;
        let latency = score_latency(ttft_ms.unwrap_or(stream_ms), stream_ms);

        let judge_scores = self.score_with_judge(question, &response).await;

        Ok(ThreadScore {
            response_text: response,
            latency,
            judge: judge_scores.ok(),
        })
    }

    async fn score_with_judge(
        &self,
        question: &Question,
        response: &str,
    ) -> Result<JudgeScores, EvalError> {
        self.cfg
            .judge
            .score(JudgeRequest {
                question: &question.text,
                context: &question.context,
                response,
                reference_answer: question.reference_answer.as_deref(),
            })
            .await
    }

    fn row_for_error(question: &Question, variant: PromptVariant, err: EvalError) -> EvalRow {
        let placeholder = ThreadScore {
            response_text: String::new(),
            latency: score_latency(0, 0),
            judge: None,
        };
        EvalRow {
            question_id: question.id.clone(),
            domain: question.domain,
            variant,
            directional: placeholder.clone(),
            depth: placeholder,
            conciseness: score_conciseness(""),
            structure: score_structure(""),
            error: Some(err.to_string()),
        }
    }
}

fn render_prompt(template: &str, question: &Question) -> String {
    let rag_text = if question.context.is_empty() {
        String::new()
    } else {
        question
            .context
            .iter()
            .enumerate()
            .map(|(i, c)| format!("[{}] {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    template
        .replace("{session_domain}", question.domain.display())
        .replace("{rag_chunks}", &rag_text)
        .replace("{rolling_summary_if_compressed}", "")
        .replace("{last_n_turns}", "")
        .replace("{question}", &question.text)
        .replace("{interviewer_role}", "Interviewer")
        .replace("{interviewer_priorities}", "")
        .replace("{role}", "Interviewer")
        .replace("{key_skills}", "")
}

/// Hash-based marker so two runs against the same prompts can be compared.
/// Reads every file under `<prompts_dir>` and hashes the concatenated bytes.
fn prompt_version_marker(prompts_dir: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    let walker = walkdir::WalkDir::new(prompts_dir)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok);
    for entry in walker {
        if entry.file_type().is_file() {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                entry.path().to_string_lossy().hash(&mut hasher);
                bytes.hash(&mut hasher);
            }
        }
    }
    format!("{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::{Category, Domain};

    #[test]
    fn render_prompt_substitutes_question_and_rag_chunks() {
        let template = "Domain: {session_domain}\nQ: {question}\nRAG:\n{rag_chunks}";
        let q = Question {
            id: "q1".into(),
            domain: Domain::SoftwareEngineering,
            category: Category::Technical,
            text: "What is Rust?".into(),
            context: vec!["Rust is memory safe.".into()],
            reference_answer: None,
        };
        let rendered = render_prompt(template, &q);
        assert!(rendered.contains("software engineering"));
        assert!(rendered.contains("What is Rust?"));
        assert!(rendered.contains("[1] Rust is memory safe."));
    }

    #[test]
    fn prompt_version_marker_is_stable_for_identical_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let first = prompt_version_marker(dir.path());
        let second = prompt_version_marker(dir.path());
        assert_eq!(first, second);
    }

    #[test]
    fn prompt_version_marker_changes_when_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let first = prompt_version_marker(dir.path());
        std::fs::write(dir.path().join("a.txt"), "goodbye").unwrap();
        let second = prompt_version_marker(dir.path());
        assert_ne!(first, second);
    }
}
