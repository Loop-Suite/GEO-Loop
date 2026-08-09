use crate::checks::{self, Metrics};
use crate::llm::Llm;
use crate::spec::Spec;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

pub const JUDGE_SYSTEM: &str = "You are a GEO (Generative Engine Optimization) judge. \
Score based on how likely a generative answer engine (ChatGPT/Perplexity/Google AI \
Overview) is to quote or excerpt this document verbatim. The document's author is \
unknown, and you do not guess at authorship. Unsupported claims, unverifiable figures, \
and ambiguous entity references are grounds for deduction. \
Do not grade leniently, and back every score with a direct quote from the document as evidence.";

/// Judging lens. Cycles each round.
/// (Repeating the same model correlates its errors, so lens separation alone doesn't
///  make the samples independent. Real independence comes from a panel of different
///  models.)
pub const LENSES: &[&str] = &[
    "Weighs overall completeness and citability in balance.",
    "Strictly checks whether the first paragraph alone answers the question and could be excerpted as-is.",
    "Especially strict about the verifiability of statistics and sources.",
    "Looks at how friendly the heading/FAQ/list structure is to excerpting-engine parsers.",
    "Checks whether the core entity (product/concept/subject) is defined without ambiguity.",
    "Looks at whether this document has a differentiator worth citing compared to competing content.",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    pub id: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub why_not_higher: String,
    pub score: f64, // 0-100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResult {
    #[serde(default)]
    pub winning_conditions: Vec<String>,
    /// self-solve-first de-anchoring: sketch the ideal document yourself before reading the actual document.
    #[serde(default)]
    pub ideal_sketch: String,
    #[serde(default)]
    pub criteria: Vec<CriterionScore>,
    #[serde(default)]
    pub improvements: Vec<String>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scored {
    pub label: String,
    /// 0-100 weighted sum
    pub total: f64,
    /// Aggregated score per criterion (0-100, trimmed mean)
    pub per_criterion: BTreeMap<String, f64>,
    /// All raw scores per criterion (per judge)
    pub raw: BTreeMap<String, Vec<f64>>,
    /// Max-min spread per criterion (judgment instability indicator)
    pub spread: BTreeMap<String, f64>,
    pub missing_sections: Vec<String>,
    /// Deterministic format/structure check results
    pub format_issues: Vec<String>,
    pub metrics: Metrics,
    pub improvements: Vec<String>,
    pub comments: Vec<String>,
    pub rounds: usize,
    pub models: Vec<String>,
}

fn judge_schema(spec: &Spec) -> serde_json::Value {
    let ids: Vec<String> = spec.criteria.iter().map(|c| c.id.clone()).collect();
    // Field order = generation order (in practice, the order of the `required` array
    // enforces this — without the preserve_order feature, serde_json serializes Object
    // as a BTreeMap so the `properties` keys get re-sorted alphabetically, but
    // `required` is a JSON array so insertion order is preserved as-is).
    // Having the model write the criteria (winning_conditions) first, before scoring,
    // reduces anchoring on the document; then having it use ideal_sketch to "sketch the
    // ideal document itself before reading the document being scored" aims for the
    // self-solve-first effect (arXiv:2607.05904: false-positive 0.719→0.012).
    json!({
        "type": "object",
        "properties": {
            "winning_conditions": {
                "type": "array",
                "minItems": 3,
                "items": {"type": "string"},
                "description": "3-6 conditions that content worth citing by a generative answer engine should meet, written before reading the document"
            },
            "ideal_sketch": {
                "type": "string",
                "minLength": 1,
                "description": "Before reading the document being scored, sketch on your own, in 100-150 \
                                 words, roughly what an ideal document would contain given this spec \
                                 (topic/audience) (self-solve-first)"
            },
            "criteria": {
                "type": "array",
                "minItems": ids.len(),
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "enum": ids},
                        "evidence": {"type": "string", "description": "Direct quote from the document (30+ characters)"},
                        "why_not_higher": {"type": "string", "description": "Why the score isn't higher"},
                        "score": {"type": "integer", "minimum": 0, "maximum": 100}
                    },
                    "required": ["id", "evidence", "why_not_higher", "score"],
                    "additionalProperties": false
                }
            },
            "improvements": {
                "type": "array", "minItems": 3, "maxItems": 8,
                "items": {"type": "string", "description": "An immediately actionable revision instruction"}
            },
            "comment": {"type": "string"}
        },
        "required": ["winning_conditions", "ideal_sketch", "criteria", "improvements", "comment"],
        "additionalProperties": false
    })
}

fn build_judge_prompt(spec: &Spec, doc: &str, lens: &str) -> String {
    format!(
        "# Task\nScore the submitted GEO document according to the judging criteria.\n\n\
         ## Document: {name}\n{ctx}\n\n\
         ## This judge's lens\n{lens}\n\n\
         ## Judging criteria (each item, integer 0-100)\n{rubric}\n\n\
         ## Score band criteria\n{bands}\n\n\
         ## Procedure\n\
         1. Before scoring the document, first write 3-6 'conditions that content worth \
         citing by a generative answer engine should meet' in winning_conditions.\n\
         2. Don't read the document being scored yet — in ideal_sketch, sketch on your \
         own, in 100-150 words, roughly 'what an ideal document should contain given \
         this spec (topic/audience)' \
         (working out your own answer first keeps you from being swayed by plausibility \
         once you see the real document).\n\
         3. Only then read the <document> below and score each item. For each item, \
         quote the document's original text directly in evidence, and write why you \
         didn't give a higher score in why_not_higher.\n\
         4. If you can't find grounds to cite, that item cannot exceed 60 points.\n\
         5. Format/length/required elements (first-paragraph length, number of \
         statistics, number of FAQs, JSON-LD validity, heading hierarchy) are handled \
         by a separate automated check, so don't factor them into scoring — evaluate \
         content quality only.\n\n\
         ## Document being scored\n<document>\n{doc}\n</document>\n",
        name = spec.name,
        ctx = spec.context,
        lens = lens,
        rubric = spec.rubric_prompt(),
        bands = spec.bands_prompt(),
        doc = doc
    )
}

/// Trimmed mean. If n>=4, drop one min and one max then average; otherwise plain mean.
/// (With many 0-100 integer samples, the median produces too many ties and fails to
/// detect small improvements.)
fn trimmed_mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    if v.len() < 4 {
        return v.iter().sum::<f64>() / v.len() as f64;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let inner = &s[1..s.len() - 1];
    inner.iter().sum::<f64>() / inner.len() as f64
}

/// Score a single document. Repeats `rounds` times, cycling through models and lenses.
pub fn score_doc(
    judges: &[Llm],
    spec: &Spec,
    label: &str,
    doc: &str,
    rounds: usize,
) -> Result<Scored> {
    anyhow::ensure!(!judges.is_empty(), "No judge models");
    let rounds = rounds.max(1);
    let schema = judge_schema(spec);
    let mut results: Vec<JudgeResult> = Vec::new();
    let mut models: Vec<String> = Vec::new();

    for i in 0..rounds {
        let llm = &judges[i % judges.len()];
        let lens = LENSES[i % LENSES.len()];
        let prompt = build_judge_prompt(spec, doc, lens);
        let v = llm
            .json(&prompt, Some(JUDGE_SYSTEM), &schema)
            .with_context(|| format!("Scoring failed ({label}, round {})", i + 1))?;
        let jr: JudgeResult = serde_json::from_value(v)
            .with_context(|| format!("Scoring result schema mismatch ({label})"))?;
        results.push(jr);
        models.push(llm.label());
    }

    let mut per_criterion: BTreeMap<String, f64> = BTreeMap::new();
    let mut raw: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut spread: BTreeMap<String, f64> = BTreeMap::new();
    for c in &spec.criteria {
        let vals: Vec<f64> = results
            .iter()
            .filter_map(|r| r.criteria.iter().find(|x| x.id == c.id))
            .map(|x| x.score.clamp(0.0, 100.0))
            .collect();
        let lo = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        spread.insert(c.id.clone(), if vals.is_empty() { 0.0 } else { hi - lo });
        per_criterion.insert(c.id.clone(), trimmed_mean(&vals));
        raw.insert(c.id.clone(), vals);
    }

    let wsum = spec.weight_sum();
    let total: f64 = spec
        .criteria
        .iter()
        .map(|c| per_criterion.get(&c.id).copied().unwrap_or(0.0) * (c.weight / wsum))
        .sum();

    let format_issues = checks::format_issues(spec, doc);
    let missing = checks::missing_sections(spec, doc);

    let mut improvements: Vec<String> = format_issues.clone();
    for r in &results {
        for imp in &r.improvements {
            let t = imp.trim().to_string();
            if !t.is_empty() && !improvements.contains(&t) {
                improvements.push(t);
            }
        }
    }

    Ok(Scored {
        label: label.to_string(),
        total: (total * 10.0).round() / 10.0,
        per_criterion,
        raw,
        spread,
        missing_sections: missing,
        format_issues,
        metrics: checks::metrics(doc),
        improvements,
        comments: results.iter().map(|r| r.comment.clone()).collect(),
        rounds,
        models,
    })
}

/// Feedback for the regeneration prompt. Does not pass along the score itself (to
/// discourage optimizing for the score).
pub fn feedback_text(s: &Scored) -> String {
    let mut out = String::from("[Revisions that must be addressed]\n");
    for i in &s.improvements {
        out.push_str(&format!("- {}\n", i));
    }
    if !s.comments.is_empty() {
        out.push_str("\n[Judges' overall comments]\n");
        for c in &s.comments {
            out.push_str(&format!("- {}\n", c));
        }
    }
    out
}

/// The 2 lowest-scoring criteria.
pub fn weak_points(spec: &Spec, s: &Scored) -> String {
    let mut v: Vec<(&str, f64)> = spec
        .criteria
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                s.per_criterion.get(&c.id).copied().unwrap_or(0.0),
            )
        })
        .collect();
    v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    v.iter()
        .take(2)
        .map(|(n, sc)| format!("- {} : {:.0}/100", n, sc))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimmed_mean_drops_outliers() {
        assert_eq!(trimmed_mean(&[70.0, 72.0, 74.0, 100.0]), 73.0);
        assert_eq!(trimmed_mean(&[80.0]), 80.0);
        assert!((trimmed_mean(&[70.0, 80.0]) - 75.0).abs() < 1e-9);
    }
}
