use crate::generate;
use crate::llm::Llm;
use crate::report;
use crate::score::{self, Scored};
use crate::spec::Spec;
use anyhow::Result;
use std::path::Path;

pub struct LoopOutcome {
    pub best_label: String,
    pub best_doc: String,
    pub best_score: Scored,
    pub first_doc: String,
    pub history: Vec<Scored>,
    pub stop_reason: String,
    /// Length inflation warning (word count growth relative to score)
    pub warnings: Vec<String>,
}

pub struct LoopCfg {
    pub target: f64,
    pub max_iter: usize,
    pub rounds: usize,
    /// If improvement over the previous best score is below this value, it's considered stagnation.
    pub min_delta: f64,
    /// Early stop if stagnation persists for this many consecutive rounds.
    pub patience: usize,
}

/// Generate → score → regenerate with feedback loop.
/// The return value is the highest-scoring round across all rounds (argmax), not the last round.
pub fn run(
    gen_llm: &Llm,
    judges: &[Llm],
    spec: &Spec,
    idea: &str,
    out_dir: &Path,
    cfg: &LoopCfg,
    angle: &str,
) -> Result<LoopOutcome> {
    let mut doc = generate::generate(gen_llm, spec, idea, angle)?;
    let mut history: Vec<Scored> = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    let mut best_i = 0usize;
    let mut stall = 0usize;
    let mut stop_reason = format!("Reached max iterations ({} rounds)", cfg.max_iter.max(1));
    // Warning for detecting a spike after stagnation (highlighted prominently in the report).
    // Based on an insight from CHERRL (arXiv:2606.04923) — that a "spike after stagnation"
    // pattern in rubric-based rewards can signal reward hacking. No new API calls are added;
    // detection relies solely on the existing per-round scores.
    let mut spike_warnings: Vec<String> = Vec::new();

    for i in 0..cfg.max_iter.max(1) {
        let label = format!("iter{:02}", i + 1);
        std::fs::write(out_dir.join(format!("{}.md", label)), &doc)?;

        let s = score::score_doc(judges, spec, &label, &doc, cfg.rounds)?;
        report::append_jsonl(out_dir, &s)?;
        println!(
            "  [{}] {:.1}/100  ({} words{})",
            label,
            s.total,
            s.metrics.words,
            if s.format_issues.is_empty() {
                String::new()
            } else {
                format!(", {} format issues", s.format_issues.len())
            }
        );

        let prev_best = history.get(best_i).map(|b: &Scored| b.total);
        let improved = match prev_best {
            None => true,
            Some(b) => s.total > b,
        };
        history.push(s.clone());
        docs.push(doc.clone());
        let stall_before = stall;
        if improved {
            let gain = s.total - prev_best.unwrap_or(f64::NEG_INFINITY);
            best_i = history.len() - 1;
            if prev_best.is_some() && gain < cfg.min_delta {
                stall += 1;
            } else {
                // Detect a large jump right after stagnation (consecutive negligible
                // improvements): if there were 2 or more consecutive stagnant rounds before
                // this one (stall_before >= 2) and this round's improvement is at least 3x
                // min_delta, warn. Threshold rationale: min_delta is the upper bound for a
                // "negligible improvement," so 3x that is treated as an unusual jump clearly
                // distinct from a stagnation streak (an arbitrary but reproducible,
                // deterministic threshold).
                if stall_before >= 2 && prev_best.is_some() && gain >= cfg.min_delta * 3.0 {
                    spike_warnings.push(format!(
                        "Spike after stagnation: {} — after {} consecutive stagnant rounds, this round gained +{:.1} points \
                         (at least 3x the +{:.1} point threshold) → possible reward hacking (overfitting to judge preferences). \
                         Be sure to verify with held-out re-scoring using --gate-model.",
                        label, stall_before, gain, cfg.min_delta
                    ));
                }
                stall = 0;
            }
        } else {
            stall += 1;
        }

        if s.total >= cfg.target && s.format_issues.is_empty() {
            stop_reason = format!("Reached target score of {:.0}", cfg.target);
            break;
        }
        if stall >= cfg.patience {
            stop_reason = format!(
                "Improvement stagnated ({} consecutive rounds below +{:.1} points)",
                cfg.patience, cfg.min_delta
            );
            break;
        }
        if i + 1 == cfg.max_iter.max(1) {
            break;
        }

        let fb = score::feedback_text(&history[history.len() - 1]);
        let weak = score::weak_points(spec, &history[history.len() - 1]);
        doc = generate::revise(gen_llm, spec, idea, &doc, &fb, &weak)?;
    }

    let best_score = history[best_i].clone();
    let best_doc = docs[best_i].clone();
    std::fs::write(out_dir.join("best.md"), &best_doc)?;

    // Length inflation canary: if word count grows excessively relative to score, suspect verbosity gaming.
    let mut warnings = spike_warnings;
    let first = &history[0];
    let d_score = best_score.total - first.total;
    let d_words = best_score.metrics.words as f64 - first.metrics.words as f64;
    let growth = if first.metrics.words > 0 {
        d_words / first.metrics.words as f64
    } else {
        0.0
    };
    if growth > 0.25 && d_score < 5.0 {
        warnings.push(format!(
            "Length canary: word count +{:.0}% but score only +{:.1} → possibly padding rather than substantive improvement",
            growth * 100.0,
            d_score
        ));
    }
    if best_i + 1 < history.len() {
        warnings.push(format!(
            "Last round ({:.1} points) is not the highest score → best.md is iter{:02}",
            history.last().map(|h| h.total).unwrap_or(0.0),
            best_i + 1
        ));
    }

    Ok(LoopOutcome {
        best_label: best_score.label.clone(),
        best_doc,
        first_doc: docs[0].clone(),
        best_score,
        history,
        stop_reason,
        warnings,
    })
}
