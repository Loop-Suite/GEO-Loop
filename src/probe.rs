//! `geo probe`: implements, in minimal scope, the idea from recomby-geo
//! (`plugins/recomby-geo/commands/02-audit.md`) of "throwing a real query at a fresh
//! sub-agent that knows nothing about the brand." It extracts the document's FAQ
//! questions as-is and calls `claude -p` with a separate system prompt, then
//! deterministically compares the simple keyword/number overlap between that answer
//! and our document's FAQ answer.
//!
//! **Limitation (don't overstate this — it's also noted in the README)**: this is not
//! proof that "a real AI search engine cites this document." It is only a very rough
//! leading signal (proxy) for how much the answer to a question asked without any
//! brand/document context overlaps with our document's answer. A low overlap doesn't
//! mean the claim is wrong either (it could even be a differentiation point), and a
//! high overlap doesn't guarantee an actual citation.

use crate::checks;
use crate::llm::Llm;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// System prompt used exclusively for probe calls. Never includes the brand name or
/// original document content — that's what makes it "a general-knowledge answer given
/// without knowing the brand."
const PROBE_SYSTEM: &str = "You are a general user. Answer the question below using only \
what you already know. No specific brand/product has been named for you. If you don't \
know, it's fine to say so. Output only the answer body, with no additional explanation \
or meta-commentary.";

/// Threshold for judging overlap (0.0-1.0). Below this, the claim is marked as "not
/// independently confirmed by external knowledge." Rationale: even two completely
/// unrelated texts overlap somewhat due to common words, so using 0 as the baseline is
/// vulnerable to noise; conversely, setting it too high (e.g. 0.5) causes false
/// negatives for correct answers that are merely phrased differently. 0.20 is kept as a
/// conservative fixed value based on "at least 1/5 of the topic words overlapping is
/// not coincidence but comes from real knowledge overlap" — noting this is a
/// deterministic approximation, not a substitute for sophisticated semantic comparison.
pub const OVERLAP_THRESHOLD: f64 = 0.20;

pub struct ProbeItem {
    pub question: String,
    pub doc_answer: String,
    pub probe_answer: String,
    /// Proportion (0.0-1.0) of the document answer's "significant tokens" that also
    /// appear in the probe answer
    pub overlap: f64,
    pub confirmed: bool,
}

/// Lowercases then splits on non-alphanumeric characters; only tokens that are at
/// least 4 characters long or contain a digit are treated as "significant tokens" (a
/// deterministic approximation to reduce particle/stopword noise — not morphological
/// analysis).
fn significant_tokens(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter(|w| w.chars().count() >= 4 || w.chars().any(|c| c.is_ascii_digit()))
        .map(|w| w.to_string())
        .collect()
}

/// Simple keyword/number overlap ratio (not sophisticated semantic comparison, deterministic).
fn keyword_overlap(doc_answer: &str, probe_answer: &str) -> f64 {
    let a = significant_tokens(doc_answer);
    if a.is_empty() {
        // If there are no claim tokens worth checking in the first place, treat it as
        // having nothing to judge and pass it.
        return 1.0;
    }
    let b = significant_tokens(probe_answer);
    let shared = a.intersection(&b).count();
    shared as f64 / a.len() as f64
}

/// For each FAQ query: make the probe call, compute the overlap, and write the report.
pub fn run(llm: &Llm, doc: &str, out_dir: &Path) -> Result<Vec<ProbeItem>> {
    let pairs = checks::extract_faq_pairs(doc);
    anyhow::ensure!(
        !pairs.is_empty(),
        "Cannot probe: the input document has no FAQ section (`Q: .../A: ...` pairs \
         under `## FAQ`). geo probe reuses FAQ questions as its target queries, so it \
         only applies to documents that have an FAQ."
    );

    let mut items = Vec::new();
    for (question, doc_answer) in pairs {
        let probe_answer = llm.text(&question, Some(PROBE_SYSTEM)).with_context(|| {
            format!("Probe call failed: {}", crate::llm::truncate(&question, 80))
        })?;
        let overlap = keyword_overlap(&doc_answer, &probe_answer);
        items.push(ProbeItem {
            question,
            doc_answer,
            probe_answer,
            overlap,
            confirmed: overlap >= OVERLAP_THRESHOLD,
        });
    }

    write_report(out_dir, &items)?;
    Ok(items)
}

fn write_report(out_dir: &Path, items: &[ProbeItem]) -> Result<PathBuf> {
    let mut md = String::from("# GEO probe report\n\n");
    md.push_str(
        "**Limitation**: this report does not prove that \"a real AI search engine cites \
         this document.\" It is only a very rough leading signal comparing the simple \
         keyword/number overlap (deterministic, not semantic comparison) between the \
         answer from a sub-agent asked only the FAQ question with zero knowledge of the \
         brand/original document, and our document's answer. A low overlap doesn't mean \
         the claim is wrong either (it could be a differentiation point).\n\n",
    );
    let confirmed_n = items.iter().filter(|i| i.confirmed).count();
    md.push_str(&format!(
        "Of {} queries, {:.0}% or higher overlap with external knowledge: {}\n\n",
        items.len(),
        OVERLAP_THRESHOLD * 100.0,
        confirmed_n
    ));
    for (i, it) in items.iter().enumerate() {
        md.push_str(&format!("## Q{}. {}\n\n", i + 1, it.question));
        md.push_str(&format!("- Overlap: {:.0}%", it.overlap * 100.0));
        if !it.confirmed {
            md.push_str(" → ⚠ This claim is not independently confirmed by external knowledge");
        }
        md.push('\n');
        md.push_str(&format!("- Document answer: {}\n", it.doc_answer));
        md.push_str(&format!(
            "- Probe (context-free) answer: {}\n\n",
            it.probe_answer
        ));
    }
    let path = out_dir.join("report.md");
    std::fs::write(&path, &md).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_is_one_when_no_significant_tokens_to_check() {
        assert_eq!(keyword_overlap("", "any answer"), 1.0);
    }

    #[test]
    fn overlap_detects_shared_stat_token() {
        let doc = "Shipping takes 2-3 days on average.";
        let probe = "Generally, the shipping period is known to be around 2-3 days.";
        let ov = keyword_overlap(doc, probe);
        assert!(ov > 0.0, "should have overlapping tokens: {ov}");
    }

    #[test]
    fn overlap_is_low_for_unrelated_texts() {
        let doc = "Refunds are available up to 100% within 7 days of payment.";
        let probe = "Today's weather is sunny with a temperature of 25 degrees.";
        let ov = keyword_overlap(doc, probe);
        assert!(ov < OVERLAP_THRESHOLD, "{ov}");
    }
}
