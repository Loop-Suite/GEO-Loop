use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Spec {
    /// Document/form name
    pub name: String,
    /// Topic, audience, and publishing context. Inserted verbatim into the prompt.
    #[serde(default)]
    pub context: String,
    /// Notes on scoring rationale. Shown in the report.
    #[serde(default)]
    pub scoring_source: String,
    /// Overall document length guide (word count). 0 means unspecified.
    #[serde(default)]
    pub total_words: usize,
    /// Approach angles for generation diversity.
    #[serde(default)]
    pub angles: Vec<String>,
    /// Score band descriptors (0-100). Uses default values if unspecified.
    #[serde(default)]
    pub bands: Vec<String>,
    /// General body sections (title, guide, word count). Checks for missing sections and word count via heading matching.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Rubric scoring items (citability/extractability/authority_signal/entity_clarity, etc.).
    pub criteria: Vec<Criterion>,
    /// Requirements for a direct answer in the first paragraph.
    #[serde(default)]
    pub answer_summary: AnswerSummarySpec,
    /// Statistics/figures requirements.
    #[serde(default)]
    pub statistics: StatisticsSpec,
    /// FAQ requirements.
    #[serde(default)]
    pub faq: FaqSpec,
    /// JSON-LD structured data requirements.
    #[serde(default)]
    pub structured_data: StructuredDataSpec,
    /// Whether to include an llms.txt snippet (per-document option).
    #[serde(default)]
    pub llms_txt: LlmsTxtSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub guide: String,
    #[serde(default)]
    pub words: usize,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Criterion {
    pub id: String,
    pub name: String,
    /// Weight. Normalized internally even if the sum isn't 1.
    pub weight: f64,
    #[serde(default)]
    pub guide: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnswerSummarySpec {
    /// Maximum word count for the first paragraph (direct answer).
    #[serde(default = "default_max_words")]
    pub max_words: usize,
    #[serde(default = "default_true")]
    pub required: bool,
}

impl Default for AnswerSummarySpec {
    fn default() -> Self {
        AnswerSummarySpec { max_words: default_max_words(), required: true }
    }
}

fn default_max_words() -> usize {
    60
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatisticsSpec {
    /// Minimum number of statistic/figure tokens in the document.
    #[serde(default = "default_min_stat")]
    pub min_count: usize,
    /// Whether to require a source (markdown link) accompanying statistics.
    #[serde(default = "default_true")]
    pub require_sourced: bool,
}

impl Default for StatisticsSpec {
    fn default() -> Self {
        StatisticsSpec { min_count: default_min_stat(), require_sourced: true }
    }
}

fn default_min_stat() -> usize {
    2
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FaqSpec {
    /// Minimum number of FAQ question-answer pairs.
    #[serde(default = "default_min_qa")]
    pub min_qa: usize,
    /// Whether to require FAQPage structure (JSON-LD). Independently of structured_data.required_types, checks for the presence of an FAQ in the document body.
    #[serde(default = "default_true")]
    pub require_faqpage: bool,
}

impl Default for FaqSpec {
    fn default() -> Self {
        FaqSpec { min_qa: default_min_qa(), require_faqpage: true }
    }
}

fn default_min_qa() -> usize {
    3
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StructuredDataSpec {
    /// List of @type values that must be included in the JSON-LD.
    #[serde(default = "default_required_types")]
    pub required_types: Vec<String>,
}

impl Default for StructuredDataSpec {
    fn default() -> Self {
        StructuredDataSpec { required_types: default_required_types() }
    }
}

fn default_required_types() -> Vec<String> {
    vec!["Article".to_string(), "FAQPage".to_string()]
}

/// llms.txt snippet option. Since the generation target is a single document rather than an entire site,
/// enabling this generates and checks an llms.txt snippet (```llms.txt code block) alongside the document.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LlmsTxtSpec {
    #[serde(default)]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub const DEFAULT_BANDS: &[&str] = &[
    "90~100: A level generative answer engines can cite and excerpt verbatim. Every claim is backed by a verifiable source, and the structure is designed with excerpting in mind.",
    "75~89: Citable level. The core answer is clear, but some evidence is unverified or the structure is somewhat scattered.",
    "60~74: Meets the minimum requirements. Information is present, but sentences are too long to excerpt or sources are lacking.",
    "40~59: Difficult to cite. Mostly abstract statements, lacking statistics/sources, no structure.",
    "0~39: Fails to meet requirements. No direct answer, or the core entity is unclear.",
];

impl Spec {
    pub fn load(path: &Path) -> Result<Spec> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read spec file: {}", path.display()))?;
        let spec: Spec = toml::from_str(&s)
            .with_context(|| format!("Failed to parse spec TOML: {}", path.display()))?;
        anyhow::ensure!(!spec.criteria.is_empty(), "criteria is empty");
        anyhow::ensure!(
            spec.criteria.iter().all(|c| c.weight > 0.0),
            "criteria weight must all be greater than 0"
        );
        let mut ids: Vec<&str> = spec.criteria.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        anyhow::ensure!(ids.len() == n, "duplicate criteria id");
        anyhow::ensure!(
            !spec.structured_data.required_types.is_empty(),
            "structured_data.required_types is empty"
        );
        Ok(spec)
    }

    pub fn weight_sum(&self) -> f64 {
        self.criteria.iter().map(|c| c.weight).sum()
    }

    pub fn bands_prompt(&self) -> String {
        if self.bands.is_empty() {
            DEFAULT_BANDS.join("\n")
        } else {
            self.bands.join("\n")
        }
    }

    pub fn sections_prompt(&self) -> String {
        if self.sections.is_empty() {
            return "(No specific sections defined — structure freely to fit the topic)".to_string();
        }
        self.sections
            .iter()
            .map(|s| {
                let mut line = format!("## {}\n- Writing guide: {}", s.title, s.guide);
                if s.words > 0 {
                    line.push_str(&format!("\n- Recommended length: about {} words", s.words));
                }
                if s.required {
                    line.push_str("\n- Required section");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn rubric_prompt(&self) -> String {
        let sum = self.weight_sum();
        self.criteria
            .iter()
            .map(|c| {
                format!(
                    "- id=\"{}\" | {} (scoring weight {:.0}%) : {}",
                    c.id,
                    c.name,
                    c.weight / sum * 100.0,
                    c.guide
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
