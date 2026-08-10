//! Deterministic checks. Anything that would just add variance if left to an LLM is handled here.
//! (Rationale: evaluation cost hierarchy — assertion/code rules → LLM judge, in that order, from cheap and stable.)
//!
//! The llms.txt required-elements validation (`llms_txt_issues`) is
//! a Rust rewrite of the logic in
//! [Auriti-Labs/geo-optimizer-skill](https://github.com/Auriti-Labs/geo-optimizer-skill)
//! (MIT)'s `geo_optimizer/core/audit_llms.py::_validate_llms_content`
//! (it keeps the original's 4 criteria: whether H1 is the first line, whether a `> `
//! blockquote summary is present, whether there's a markdown link, and whether the length
//! meets the minimum). The code was not ported verbatim — it was reimplemented to fit
//! this project's data structures.

use crate::spec::Spec;
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Metrics {
    pub words: usize,
    pub chars: usize,
    /// Number of tokens containing digits (approximation of stats/figures)
    pub stat_tokens: usize,
    /// Number of markdown `[text](url)` links
    pub source_links: usize,
    /// Number of FAQ Q&A pairs detected via the "Q:"/"A:" pattern
    pub faq_qa_count: usize,
    /// Whether an explicit FAQ heading (H2/H3) is present
    pub has_faq_heading: bool,
    /// Number of syntactically valid JSON-LD blocks
    pub jsonld_blocks: usize,
    /// All `@type` values found in JSON-LD (deduplicated)
    pub jsonld_types: Vec<String>,
}

/// Normalizes a heading/title for fuzzy matching: strips whitespace and lowercases, so a
/// document heading that differs from the spec's section title only in case (e.g. "## overview"
/// vs. spec title "Overview") still matches. Mirrors the case-insensitive convention `faq_metrics`
/// already uses for FAQ heading matching below.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// Replaces lines inside ``` code fences with blank lines. This keeps the heading/stats/link/FAQ
/// scans from mistaking `#`/numbers/links inside example code blocks for real document structure.
/// JSON-LD/llms.txt extraction needs the content "inside" the fence, so it scans the raw `doc` directly instead of using this function.
fn strip_code_fences(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len());
    let mut in_fence = false;
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[derive(Debug)]
pub struct Heading {
    pub level: usize,
    pub text: String,
}

/// Extracts (level, text) from a line starting with `#`.
pub fn parse_headings(doc: &str) -> Vec<Heading> {
    doc.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            if t.starts_with('#') {
                let level = t.chars().take_while(|&c| c == '#').count();
                let text = t.trim_start_matches('#').trim().to_string();
                Some(Heading { level, text })
            } else {
                None
            }
        })
        .collect()
}

/// Splits into (heading, body) pairs based on headings starting with `#`.
pub fn split_sections(doc: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut cur_head = String::new();
    let mut cur_body = String::new();
    for line in doc.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            if !cur_head.is_empty() || !cur_body.trim().is_empty() {
                out.push((cur_head.clone(), cur_body.clone()));
            }
            cur_head = t.trim_start_matches('#').trim().to_string();
            cur_body.clear();
        } else {
            cur_body.push_str(line);
            cur_body.push('\n');
        }
    }
    if !cur_head.is_empty() || !cur_body.trim().is_empty() {
        out.push((cur_head, cur_body));
    }
    out
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Determines whether a line is an H1 starting with exactly one `#`. In addition to `# Title`,
/// a form without a space like `#Title` is also accepted as H1 (strictly requiring "# " would
/// produce a false positive of "no direct answer in the first paragraph" whenever an LLM
/// drops the space).
fn is_h1(line: &str) -> bool {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    hashes == 1 && t.len() > hashes
}

/// The paragraph text from after H1 (or from the start of the document) up to the first blank line.
/// Used for a heuristic check (presence + length) of whether a direct-answer sentence exists.
/// Scans fence-stripped text, like every other structural scanner in this file — otherwise a
/// single-`#` comment line inside a fenced code example (e.g. a Python/Shell/YAML `# comment`)
/// would be mistaken for the document's H1.
pub fn first_paragraph(doc: &str) -> String {
    let doc = strip_code_fences(doc);
    let has_h1 = doc.lines().any(is_h1);
    let mut seen_h1 = !has_h1;
    let mut collected: Vec<String> = Vec::new();
    for line in doc.lines() {
        let t = line.trim();
        if !seen_h1 {
            if is_h1(t) {
                seen_h1 = true;
            }
            continue;
        }
        if t.is_empty() {
            if collected.is_empty() {
                continue;
            } else {
                break;
            }
        }
        if t.starts_with('#') || t.starts_with("```") {
            break;
        }
        collected.push(t.to_string());
    }
    collected.join(" ")
}

/// Heading hierarchy consistency (H1→H2→H3, no skipping levels). Code fence interiors are ignored.
pub fn heading_hierarchy_issues(doc: &str) -> Vec<String> {
    let heads = parse_headings(&strip_code_fences(doc));
    let mut issues = Vec::new();
    let mut prev_level = 0usize;
    for h in &heads {
        if prev_level > 0 && h.level > prev_level + 1 {
            issues.push(format!(
                "Heading level skipped: H{} followed directly by H{} ('{}') → insert an intermediate-level heading",
                prev_level, h.level, h.text
            ));
        }
        prev_level = h.level;
    }
    issues
}

/// Count of whitespace-separated tokens containing digits (approximation of stats/figures). Catches things like "43%", "2.5x", "1,200 people".
pub fn stat_token_count(doc: &str) -> usize {
    doc.split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
        .count()
}

/// Count of markdown `[text](url)` links.
pub fn source_link_count(doc: &str) -> usize {
    link_regex().find_iter(doc).count()
}

fn link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap())
}

/// Strips markdown bold markers (`**`) from a line before FAQ label matching. A plain
/// `trim_start_matches("**")`/`trim_end_matches("**")` only removes `**` sitting at the
/// very start/end of the whole line, so a common bold-label format like `**Q:** text`
/// (where `**` wraps just the label, not the whole line) leaves the trailing `**` stuck
/// to the captured text (e.g. "** text" instead of "text"). Removing every `**`
/// occurrence, regardless of position, handles that case as well as `**Q**: text`.
fn strip_bold_markers(s: &str) -> String {
    s.replace("**", "")
}

/// Whether an FAQ heading is present + count of Q&A pairs matching the "Q:"/"A:" pattern.
pub fn faq_metrics(doc: &str) -> (bool, usize) {
    let heads = parse_headings(doc);
    let has_faq_heading = heads.iter().any(|h| {
        let low = h.text.to_lowercase();
        low.contains("faq") || low.contains("자주 묻는 질문") || low.contains("질문과 답변")
    });

    static Q_RE: OnceLock<Regex> = OnceLock::new();
    static A_RE: OnceLock<Regex> = OnceLock::new();
    let q_re = Q_RE.get_or_init(|| Regex::new(r"(?i)^q[:.]\s*\S").unwrap());
    let a_re = A_RE.get_or_init(|| Regex::new(r"(?i)^a[:.]\s*\S").unwrap());
    let mut qa_count = 0usize;
    let mut pending_q = false;
    for line in doc.lines() {
        let t = line.trim_start_matches('#').trim();
        let t = strip_bold_markers(t);
        let t = t.trim();
        if q_re.is_match(t) {
            pending_q = true;
        } else if pending_q && a_re.is_match(t) {
            qa_count += 1;
            pending_q = false;
        }
    }
    (has_faq_heading, qa_count)
}

/// Reuses the same "Q:"/"A:" recognition rules as `faq_metrics`, but returns the actual
/// question/answer text pairs instead of a count. Used by the `geo probe` subcommand to extract target queries.
/// Strips code fences internally (unlike `faq_metrics`, whose only caller pre-strips via
/// `metrics()`) so a fenced example that happens to contain `Q:`/`A:`-shaped lines — e.g.
/// documentation showing the FAQ format — is never picked up as a real FAQ entry.
pub fn extract_faq_pairs(doc: &str) -> Vec<(String, String)> {
    let doc = strip_code_fences(doc);
    static Q_RE: OnceLock<Regex> = OnceLock::new();
    static A_RE: OnceLock<Regex> = OnceLock::new();
    let q_re = Q_RE.get_or_init(|| Regex::new(r"(?i)^q[:.]\s*(\S.*)$").unwrap());
    let a_re = A_RE.get_or_init(|| Regex::new(r"(?i)^a[:.]\s*(\S.*)$").unwrap());
    let mut pairs = Vec::new();
    let mut pending_q: Option<String> = None;
    for line in doc.lines() {
        let t = line.trim_start_matches('#').trim();
        let t = strip_bold_markers(t);
        let t = t.trim();
        if let Some(cap) = q_re.captures(t) {
            pending_q = Some(cap[1].trim().to_string());
        } else if let Some(cap) = a_re.captures(t) {
            if let Some(q) = pending_q.take() {
                pairs.push((q, cap[1].trim().to_string()));
            }
        }
    }
    pairs
}

/// Scans code fences line by line and extracts only the contents of fences whose language tag
/// matches exactly. (Unlike a regex's lazy `.*?`, this doesn't misbehave even if a "```" string
/// happens to appear in the middle of fence content (not at the start of a line) — fence
/// boundaries are determined solely by "does the line start with ```". The language tag must
/// match `lang_tag` exactly ("```jsonc" does not match "json".)
fn extract_fenced_blocks(doc: &str, lang_tag: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut collecting = false;
    let mut in_other_fence = false;
    let mut current = String::new();
    for line in doc.lines() {
        let t = line.trim_start();
        if !collecting && !in_other_fence {
            if let Some(after) = t.strip_prefix("```") {
                if after.trim() == lang_tag {
                    collecting = true;
                    current.clear();
                } else {
                    in_other_fence = true;
                }
                continue;
            }
        } else if t.starts_with("```") {
            if collecting {
                blocks.push(std::mem::take(&mut current));
            }
            collecting = false;
            in_other_fence = false;
            continue;
        }
        if collecting {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
}

/// Extracts JSON-LD candidate text from ```json code blocks and `<script type="application/ld+json">` blocks.
pub fn extract_jsonld_blocks(doc: &str) -> Vec<String> {
    let mut blocks = extract_fenced_blocks(doc, "json");
    let script_re =
        Regex::new(r#"(?s)<script[^>]*type="application/ld\+json"[^>]*>(.*?)</script>"#).unwrap();
    for cap in script_re.captures_iter(doc) {
        blocks.push(cap[1].to_string());
    }
    blocks
}

/// Recursively collects all `@type` values (string or array) from a JSON value tree.
/// The recursion depth is capped to guard against LLM output (a low-trust input source) producing abnormally deep JSON.
const MAX_JSON_DEPTH: usize = 64;

pub fn collect_types(v: &serde_json::Value, out: &mut BTreeSet<String>) {
    collect_types_bounded(v, out, 0);
}

fn collect_types_bounded(v: &serde_json::Value, out: &mut BTreeSet<String>, depth: usize) {
    if depth > MAX_JSON_DEPTH {
        return;
    }
    match v {
        serde_json::Value::Object(map) => {
            if let Some(t) = map.get("@type") {
                match t {
                    serde_json::Value::String(s) => {
                        out.insert(s.clone());
                    }
                    serde_json::Value::Array(arr) => {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                out.insert(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            for val in map.values() {
                collect_types_bounded(val, out, depth + 1);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_types_bounded(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Extracts a ```llms.txt code block.
pub fn extract_llms_txt_snippet(doc: &str) -> Option<String> {
    extract_fenced_blocks(doc, "llms.txt").into_iter().next()
}

/// Validates the required elements of an llms.txt snippet.
/// Ported from: Auriti-Labs/geo-optimizer-skill (MIT) `audit_llms.py::_validate_llms_content`.
/// The original checks 4 things — (1) whether H1 is the first non-empty line, (2) whether a
/// `> ` blockquote summary is present, (3) whether there's a markdown link, (4) whether the
/// length is at least the minimum. This function reimplements that judgment logic in Rust
/// (the original uses a 100-word threshold; here it's lowered to 20 since this is a per-document snippet).
pub fn llms_txt_issues(snippet: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let lines: Vec<&str> = snippet.lines().collect();
    let non_empty: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();

    match non_empty.first() {
        Some(first) if first.trim_start().starts_with("# ") => {}
        Some(_) => issues.push("llms.txt: H1('# ') is not the first line".to_string()),
        None => {
            issues.push("llms.txt: no content".to_string());
            return issues;
        }
    }

    if !lines.iter().any(|l| l.trim_start().starts_with("> ")) {
        issues.push("llms.txt: missing '> ' summary blockquote".to_string());
    }
    if !link_regex().is_match(snippet) {
        issues.push("llms.txt: missing markdown link list".to_string());
    }
    let wc = word_count(snippet);
    if wc < 20 {
        issues.push(format!("llms.txt: content too short ({} words)", wc));
    }
    issues
}

pub fn metrics(doc: &str) -> Metrics {
    // The heading/stats/link/FAQ scans exclude the interior of code fences (example JSON-LD,
    // llms.txt snippets) — otherwise numbers/links/fake headings inside example blocks would
    // get miscounted as real body structure.
    // Only JSON-LD extraction needs the content "inside" the fence, so it uses the raw doc as-is.
    let scan = strip_code_fences(doc);
    let (has_faq_heading, faq_qa_count) = faq_metrics(&scan);
    let blocks = extract_jsonld_blocks(doc);
    let mut types: BTreeSet<String> = BTreeSet::new();
    let mut valid_jsonld = 0usize;
    for b in &blocks {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(b) {
            valid_jsonld += 1;
            collect_types(&v, &mut types);
        }
    }
    Metrics {
        words: word_count(&scan),
        chars: doc.chars().count(),
        stat_tokens: stat_token_count(&scan),
        source_links: source_link_count(&scan),
        faq_qa_count,
        has_faq_heading,
        jsonld_blocks: valid_jsonld,
        jsonld_types: types.into_iter().collect(),
    }
}

/// Missing required generic section titles. Code fence interiors are ignored.
pub fn missing_sections(spec: &Spec, doc: &str) -> Vec<String> {
    let heads: Vec<String> = split_sections(&strip_code_fences(doc))
        .into_iter()
        .map(|(h, _)| norm(&h))
        .collect();
    spec.sections
        .iter()
        .filter(|s| {
            let want = norm(&s.title);
            s.required
                && !heads
                    .iter()
                    .any(|h| !h.is_empty() && (h.contains(&want) || want.contains(h)))
        })
        .map(|s| s.title.clone())
        .collect()
}

fn answer_summary_issues(spec: &Spec, doc: &str) -> Vec<String> {
    let mut issues = Vec::new();
    if !spec.answer_summary.required {
        return issues;
    }
    let para = first_paragraph(doc);
    let wc = word_count(&para);
    if para.trim().is_empty() {
        issues.push(
            "No direct-answer sentence in the first paragraph → place a direct answer to the key question as the opening sentence"
                .to_string(),
        );
    } else if wc > spec.answer_summary.max_words {
        issues.push(format!(
            "First paragraph is {} words, exceeding the target of {} words → tighten the first sentence into a more concise direct answer",
            wc, spec.answer_summary.max_words
        ));
    } else if wc < 8 {
        issues.push(format!(
            "First paragraph is only {} words, too short → strengthen it into a complete direct-answer sentence for the question",
            wc
        ));
    }
    issues
}

fn statistics_issues(spec: &Spec, m: &Metrics) -> Vec<String> {
    let mut issues = Vec::new();
    if m.stat_tokens < spec.statistics.min_count {
        issues.push(format!(
            "{} stat/figure tokens → at least {} required",
            m.stat_tokens, spec.statistics.min_count
        ));
    }
    if spec.statistics.require_sourced && m.source_links == 0 {
        issues.push("No sources (markdown links) → add supporting evidence for stats/claims in [source](URL) form".to_string());
    }
    issues
}

fn faq_issues(spec: &Spec, m: &Metrics) -> Vec<String> {
    let mut issues = Vec::new();
    if !m.has_faq_heading && m.faq_qa_count == 0 {
        issues.push("No FAQ section → add an 'FAQ' heading and 'Q: .../A: ...' pairs".to_string());
    } else if m.faq_qa_count < spec.faq.min_qa {
        issues.push(format!(
            "{} FAQ Q&A pairs → at least {} required",
            m.faq_qa_count, spec.faq.min_qa
        ));
    }
    issues
}

fn structured_data_issues(spec: &Spec, doc: &str) -> Vec<String> {
    let blocks = extract_jsonld_blocks(doc);
    let mut issues = Vec::new();
    if blocks.is_empty() {
        issues.push(
            "No JSON-LD structured data block → add schema.org markup in a ```json code block"
                .to_string(),
        );
        return issues;
    }
    let mut all_types: BTreeSet<String> = BTreeSet::new();
    for b in &blocks {
        match serde_json::from_str::<serde_json::Value>(b) {
            Ok(v) => {
                collect_types(&v, &mut all_types);
                issues.extend(schema_field_issues(&v));
            }
            Err(e) => issues.push(format!(
                "Failed to parse JSON-LD block (syntax error): {}",
                crate::llm::truncate(&e.to_string(), 120)
            )),
        }
    }
    for req in &spec.structured_data.required_types {
        if !all_types.contains(req) {
            issues.push(format!("JSON-LD is missing required @type '{}'", req));
        }
    }
    issues
}

/// Checks required/recommended fields for FAQPage/Article/Product/HowTo JSON-LD (field
/// presence, not structural syntax).
/// Basis: Google Search Central structured data guidelines
/// (https://developers.google.com/search/docs/appearance/structured-data/faqpage,
///  https://developers.google.com/search/docs/appearance/structured-data/article,
///  https://developers.google.com/search/docs/appearance/structured-data/product,
///  https://developers.google.com/search/docs/appearance/structured-data/how-to) —
/// schema.org itself is an optional vocabulary with no "required" concept; this required/recommended distinction comes from Google's documentation.
fn schema_field_issues(v: &serde_json::Value) -> Vec<String> {
    let mut issues = Vec::new();
    check_schema_node(v, &mut issues, 0);
    issues
}

fn check_schema_node(v: &serde_json::Value, issues: &mut Vec<String>, depth: usize) {
    if depth > MAX_JSON_DEPTH {
        return;
    }
    if let serde_json::Value::Object(map) = v {
        if let Some(t) = map.get("@type").and_then(|t| t.as_str()) {
            match t {
                "FAQPage" => match map.get("mainEntity").and_then(|m| m.as_array()) {
                    Some(arr) if !arr.is_empty() => {
                        for (i, q) in arr.iter().enumerate() {
                            let has_name = q
                                .get("name")
                                .and_then(|n| n.as_str())
                                .map(|s| !s.trim().is_empty())
                                .unwrap_or(false);
                            let has_answer = q
                                .get("acceptedAnswer")
                                .and_then(|a| a.get("text"))
                                .and_then(|t| t.as_str())
                                .map(|s| !s.trim().is_empty())
                                .unwrap_or(false);
                            if !has_name {
                                issues.push(format!("FAQPage mainEntity[{i}] is missing name (question text) (required by Google)"));
                            }
                            if !has_answer {
                                issues.push(format!(
                                    "FAQPage mainEntity[{i}] is missing acceptedAnswer.text (answer text) (required by Google)"
                                ));
                            }
                        }
                    }
                    _ => issues.push("FAQPage's mainEntity array is missing or empty (required field per Google)".to_string()),
                },
                "Article" | "BlogPosting" | "NewsArticle" => {
                    for (field, required) in [
                        ("headline", true),
                        ("image", true),
                        ("datePublished", true),
                        ("author", true),
                        ("dateModified", false),
                        ("publisher", false),
                        ("mainEntityOfPage", false),
                    ] {
                        let present = map.get(field).map(|f| !f.is_null()).unwrap_or(false);
                        if !present {
                            let tag = if required { "required" } else { "recommended" };
                            issues.push(format!("{t} JSON-LD is missing '{field}' ({tag}, per Google structured data guidelines)"));
                        }
                    }
                }
                "Product" => {
                    for (field, required) in [
                        ("name", true),
                        ("image", true),
                        ("description", false),
                        ("sku", false),
                        ("offers", false),
                    ] {
                        let present = map.get(field).map(|f| !f.is_null()).unwrap_or(false);
                        if !present {
                            let tag = if required { "required" } else { "recommended" };
                            issues.push(format!("{t} JSON-LD is missing '{field}' ({tag}, per Google structured data guidelines)"));
                        }
                    }
                }
                "HowTo" => {
                    for (field, required) in [
                        ("name", true),
                        ("step", true),
                        ("totalTime", false),
                        ("tool", false),
                        ("supply", false),
                    ] {
                        let present = map.get(field).map(|f| !f.is_null()).unwrap_or(false);
                        if !present {
                            let tag = if required { "required" } else { "recommended" };
                            issues.push(format!("{t} JSON-LD is missing '{field}' ({tag}, per Google structured data guidelines)"));
                        }
                    }
                }
                _ => {}
            }
        }
        for val in map.values() {
            check_schema_node(val, issues, depth + 1);
        }
    } else if let serde_json::Value::Array(arr) = v {
        for item in arr {
            check_schema_node(item, issues, depth + 1);
        }
    }
}

/// All deterministic format/structure-related issues.
pub fn format_issues(spec: &Spec, doc: &str) -> Vec<String> {
    let mut issues: Vec<String> = Vec::new();

    for m in missing_sections(spec, doc) {
        issues.push(format!("Missing required section '{}' → add it", m));
    }

    let secs = split_sections(&strip_code_fences(doc));
    for s in &spec.sections {
        if s.words == 0 {
            continue;
        }
        let want = norm(&s.title);
        if let Some((_, body)) = secs
            .iter()
            .find(|(h, _)| !h.is_empty() && (norm(h).contains(&want) || want.contains(&norm(h))))
        {
            let n = word_count(body);
            let lo = (s.words as f64 * 0.6) as usize;
            let hi = (s.words as f64 * 1.8) as usize;
            if n < lo {
                issues.push(format!(
                    "'{}' is too short: {} words (recommended {} words) → add supporting evidence/examples",
                    s.title, n, s.words
                ));
            } else if n > hi {
                issues.push(format!(
                    "'{}' is too long: {} words (recommended {} words) → condense",
                    s.title, n, s.words
                ));
            }
        }
    }

    let m = metrics(doc);
    if spec.total_words > 0 {
        let lo = (spec.total_words as f64 * 0.7) as usize;
        let hi = (spec.total_words as f64 * 1.3) as usize;
        if m.words < lo {
            issues.push(format!(
                "Total length too short: {} words (target {} words)",
                m.words, spec.total_words
            ));
        } else if m.words > hi {
            issues.push(format!(
                "Total length too long: {} words (target {} words)",
                m.words, spec.total_words
            ));
        }
    }

    issues.extend(answer_summary_issues(spec, doc));
    issues.extend(statistics_issues(spec, &m));
    issues.extend(faq_issues(spec, &m));
    issues.extend(structured_data_issues(spec, doc));
    issues.extend(heading_hierarchy_issues(doc));

    if spec.llms_txt.enabled {
        match extract_llms_txt_snippet(doc) {
            Some(snippet) => issues.extend(llms_txt_issues(&snippet)),
            None => issues.push(
                "No llms.txt snippet (option enabled) → add one in a ```llms.txt code block"
                    .to_string(),
            ),
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{
        AnswerSummarySpec, Criterion, FaqSpec, LlmsTxtSpec, Section, StatisticsSpec,
        StructuredDataSpec,
    };

    fn min_spec() -> Spec {
        Spec {
            name: "test".into(),
            context: String::new(),
            scoring_source: String::new(),
            total_words: 0,
            angles: vec![],
            bands: vec![],
            sections: vec![],
            criteria: vec![Criterion {
                id: "citability".into(),
                name: "Citability".into(),
                weight: 1.0,
                guide: String::new(),
            }],
            answer_summary: AnswerSummarySpec::default(),
            statistics: StatisticsSpec::default(),
            faq: FaqSpec::default(),
            structured_data: StructuredDataSpec::default(),
            llms_txt: LlmsTxtSpec::default(),
        }
    }

    #[test]
    fn missing_sections_matches_heading_case_insensitively() {
        // The doc's heading differs from the spec's section title only in case; it must
        // still count as present, matching faq_metrics's case-insensitive convention.
        let mut spec = min_spec();
        spec.sections = vec![Section {
            id: "overview".into(),
            title: "Overview".into(),
            guide: String::new(),
            words: 0,
            required: true,
        }];
        let doc = "# T\n\n## overview\n\nBody text here.\n";
        let missing = missing_sections(&spec, doc);
        assert!(missing.is_empty(), "{:?}", missing);
    }

    #[test]
    fn detects_faq_qa_pairs() {
        let doc = "# T\n\n## FAQ\n\nQ: Question1?\nA: Answer1\n\nQ: Question2?\nA: Answer2\n";
        let (has_heading, count) = faq_metrics(doc);
        assert!(has_heading);
        assert_eq!(count, 2);
    }

    #[test]
    fn counts_source_links() {
        let doc = "Body text [source1](https://a.com) and [source2](https://b.com)";
        assert_eq!(source_link_count(doc), 2);
    }

    #[test]
    fn heading_skip_detected() {
        let doc = "# H1\n\n### H3 skip\n";
        let issues = heading_hierarchy_issues(doc);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn jsonld_type_collection() {
        let doc = "```json\n{\"@context\":\"https://schema.org\",\"@type\":\"Article\"}\n```";
        let m = metrics(doc);
        assert_eq!(m.jsonld_blocks, 1);
        assert!(m.jsonld_types.contains(&"Article".to_string()));
    }

    #[test]
    fn llms_txt_snippet_ok() {
        let snippet = "# Site\n\n> A one line summary that is reasonably descriptive of the site.\n\n- [Home](https://example.com) more words to pad length above twenty total words here now\n";
        let issues = llms_txt_issues(snippet);
        assert!(issues.is_empty(), "{:?}", issues);
    }

    #[test]
    fn answer_summary_first_paragraph() {
        let doc = "# Title\n\nThis is a direct answer sentence that responds to the query clearly.\n\n## Next\nbody";
        let spec = min_spec();
        let issues = answer_summary_issues(&spec, doc);
        assert!(issues.is_empty(), "{:?}", issues);
    }

    #[test]
    fn first_paragraph_ignores_hash_comment_inside_code_fence() {
        // A document with no real H1 but a fenced shell example containing a "# comment"
        // line must not have that fenced comment mistaken for the document's H1 — the
        // real opening paragraph should still be returned.
        let doc =
            "This is the real opening paragraph that directly answers the question in full.\n\n\
                    ```bash\n# Install dependencies\npip install foo\n```\n\n\
                    More trailing body text here.\n";
        let para = first_paragraph(doc);
        assert!(
            para.contains("real opening paragraph"),
            "expected the real opening paragraph, got {:?}",
            para
        );
    }

    #[test]
    fn word_count_handles_korean_by_whitespace() {
        // Korean text is counted by eojeol (whitespace-separated) units — not morphological analysis, an intentional approximation.
        assert_eq!(word_count("이것은 다섯 어절로 된 한국어 문장입니다"), 6);
    }

    #[test]
    fn faq_heading_present_but_no_qa_pairs_is_flagged() {
        let mut spec = min_spec();
        spec.faq.min_qa = 1;
        let doc = "# T\n\n## FAQ\n\nFrequently asked questions are coming soon.\n";
        let (has_heading, qa_count) = faq_metrics(doc);
        assert!(has_heading);
        assert_eq!(qa_count, 0);
        let m = metrics(doc);
        let issues = faq_issues(&spec, &m);
        assert!(!issues.is_empty(), "{:?}", issues);
    }

    #[test]
    fn embedded_triple_backtick_inside_json_string_does_not_truncate_block() {
        // Must not be mistaken for a fence even if ``` appears in the middle of a value string (not at the start of a line).
        let doc = "```json\n{\"@type\":\"Article\",\"note\":\"example: ```code``` inline\"}\n```";
        let blocks = extract_jsonld_blocks(doc);
        assert_eq!(blocks.len(), 1);
        assert!(
            serde_json::from_str::<serde_json::Value>(&blocks[0]).is_ok(),
            "{:?}",
            blocks
        );
    }

    #[test]
    fn jsonc_fence_is_not_mistaken_for_jsonld() {
        // A fence with a different language tag like "```jsonc" must not be mismatched as a JSON-LD block.
        let doc = "```jsonc\n// example config, not JSON-LD\n{\"foo\": 1}\n```";
        let blocks = extract_jsonld_blocks(doc);
        assert!(blocks.is_empty(), "{:?}", blocks);
    }

    #[test]
    fn code_fence_example_heading_and_stats_are_excluded_from_scan() {
        let doc = "# Real Title\n\nReal direct answer paragraph with enough words to pass the check here now.\n\n```\n# This is an example heading, not a real heading\nStat example: numbers like 43% should also be ignored\n```\n";
        let heads = parse_headings(&strip_code_fences(doc));
        assert_eq!(heads.len(), 1, "{:?}", heads);
        assert_eq!(heads[0].text, "Real Title");
        let m = metrics(doc);
        assert_eq!(
            m.stat_tokens, 0,
            "Numbers inside fences must not be counted as stats: {}",
            m.stat_tokens
        );
    }

    #[test]
    fn faqpage_missing_main_entity_is_flagged() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"@context":"https://schema.org","@type":"FAQPage"}"#).unwrap();
        let issues = schema_field_issues(&v);
        assert!(
            issues.iter().any(|i| i.contains("mainEntity")),
            "{:?}",
            issues
        );
    }

    #[test]
    fn article_missing_headline_is_flagged() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"@context":"https://schema.org","@type":"Article","author":"me"}"#,
        )
        .unwrap();
        let issues = schema_field_issues(&v);
        assert!(
            issues.iter().any(|i| i.contains("headline")),
            "{:?}",
            issues
        );
    }

    #[test]
    fn extract_faq_pairs_returns_question_and_answer_text() {
        let doc = "# T\n\n## FAQ\n\nQ: How many days does shipping take?\nA: It takes an average of 2-3 days.\n\nQ: Can I get a refund?\nA: Unconditional refund within 7 days.\n";
        let pairs = extract_faq_pairs(doc);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "How many days does shipping take?");
        assert_eq!(pairs[0].1, "It takes an average of 2-3 days.");
    }

    #[test]
    fn extract_faq_pairs_ignores_qa_inside_code_fence() {
        // geo probe calls extract_faq_pairs directly on the raw document. A fenced example
        // showing the FAQ format (e.g. documentation) must not be extracted as a real pair.
        let doc = "# T\n\nIntro text.\n\n\
                    ```markdown\nQ: Example question in a code sample?\nA: Example answer in a code sample.\n```\n\n\
                    ## FAQ\n\nQ: Real question?\nA: Real answer.\n";
        let pairs = extract_faq_pairs(doc);
        assert_eq!(pairs.len(), 1, "{:?}", pairs);
        assert_eq!(pairs[0].0, "Real question?");
    }

    #[test]
    fn extract_faq_pairs_strips_bold_markdown_labels() {
        let doc = "# T\n\n## FAQ\n\n**Q:** What is X?\n**A:** X is Y.\n\n**Q**: Another question?\n**A**: Another answer.\n";
        let pairs = extract_faq_pairs(doc);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "What is X?");
        assert_eq!(pairs[0].1, "X is Y.");
        assert_eq!(pairs[1].0, "Another question?");
        assert_eq!(pairs[1].1, "Another answer.");
    }

    #[test]
    fn faq_metrics_counts_bold_markdown_qa_pairs() {
        let doc = "# T\n\n## FAQ\n\n**Q:** What is X?\n**A:** X is Y.\n";
        let (has_heading, count) = faq_metrics(doc);
        assert!(has_heading);
        assert_eq!(count, 1);
    }

    #[test]
    fn product_missing_required_fields_is_flagged() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"@context":"https://schema.org","@type":"Product","description":"A short description"}"#).unwrap();
        let issues = schema_field_issues(&v);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("name") && i.contains("required")),
            "{:?}",
            issues
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("image") && i.contains("required")),
            "{:?}",
            issues
        );
    }

    #[test]
    fn howto_missing_required_fields_is_flagged() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"@context":"https://schema.org","@type":"HowTo","totalTime":"PT30M"}"#,
        )
        .unwrap();
        let issues = schema_field_issues(&v);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("name") && i.contains("required")),
            "{:?}",
            issues
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("step") && i.contains("required")),
            "{:?}",
            issues
        );
    }

    // --- Edge cases: empty input ---------------------------------------------------

    #[test]
    fn empty_doc_metrics_are_all_zero_no_panic() {
        let m = metrics("");
        assert_eq!(m.words, 0);
        assert_eq!(m.chars, 0);
        assert_eq!(m.stat_tokens, 0);
        assert_eq!(m.source_links, 0);
        assert_eq!(m.faq_qa_count, 0);
        assert!(!m.has_faq_heading);
        assert_eq!(m.jsonld_blocks, 0);
        assert!(m.jsonld_types.is_empty());
    }

    #[test]
    fn empty_doc_first_paragraph_is_empty_string() {
        assert_eq!(first_paragraph(""), "");
    }

    #[test]
    fn empty_doc_structural_scans_return_empty_no_panic() {
        assert!(parse_headings("").is_empty());
        assert!(split_sections("").is_empty());
        assert!(heading_hierarchy_issues("").is_empty());
        assert!(extract_faq_pairs("").is_empty());
        assert!(extract_jsonld_blocks("").is_empty());
        assert_eq!(extract_llms_txt_snippet(""), None);
    }

    #[test]
    fn empty_doc_format_issues_flags_requirements_without_panicking() {
        // An empty document should surface issues (missing FAQ/structured-data/answer),
        // not panic or silently pass.
        let spec = min_spec();
        let issues = format_issues(&spec, "");
        assert!(!issues.is_empty(), "{:?}", issues);
    }

    #[test]
    fn empty_snippet_llms_txt_issues_reports_no_content() {
        let issues = llms_txt_issues("");
        assert_eq!(issues, vec!["llms.txt: no content".to_string()]);
    }

    #[test]
    fn whitespace_only_doc_behaves_like_empty_no_panic() {
        let doc = "   \n\t\n   \n";
        let m = metrics(doc);
        assert_eq!(m.words, 0);
        assert_eq!(first_paragraph(doc), "");
    }

    // --- Edge cases: huge documents -------------------------------------------------

    #[test]
    fn huge_document_metrics_scale_correctly_no_panic() {
        // ~50k lines / a few MB of text — checks that scanning is linear, correct, and
        // doesn't panic or overflow on a document far larger than any real GEO doc.
        let mut doc = String::from("# Title\n\nReal direct answer paragraph here now.\n\n");
        for i in 0..50_000 {
            doc.push_str(&format!(
                "Padding line number {i} with stat {i}% included.\n"
            ));
        }
        doc.push_str("\n## FAQ\n\nQ: Real question?\nA: Real answer.\n");
        let m = metrics(&doc);
        assert_eq!(m.words, word_count(&doc));
        assert_eq!(m.stat_tokens, stat_token_count(&doc));
        assert_eq!(m.faq_qa_count, 1);
        assert!(m.has_faq_heading);
    }

    #[test]
    fn huge_single_line_document_does_not_panic() {
        // A single pathologically long line (no newlines at all).
        let doc: String = "word ".repeat(500_000);
        let m = metrics(&doc);
        assert_eq!(m.words, 500_000);
        assert!(heading_hierarchy_issues(&doc).is_empty());
    }

    #[test]
    fn deeply_nested_but_narrow_heading_list_does_not_panic() {
        // Thousands of headings in sequence (not nested JSON, but a structural stress
        // test for parse_headings/heading_hierarchy_issues on a huge heading count).
        let mut doc = String::new();
        for i in 0..20_000 {
            doc.push_str(&format!("# H{i}\n"));
        }
        let heads = parse_headings(&doc);
        assert_eq!(heads.len(), 20_000);
        // All at the same level (H1), so no "skipped level" issues.
        assert!(heading_hierarchy_issues(&doc).is_empty());
    }

    // --- Edge cases: malformed / adversarial JSON-LD ---------------------------------

    #[test]
    fn jsonld_exceeding_serde_recursion_limit_reports_parse_error_not_panic() {
        // serde_json enforces its own ~128-level recursion limit and returns Err rather
        // than overflowing the stack, but this pins down that our error path (a reported
        // format issue, not a crash) is what actually happens for adversarially deep
        // JSON-LD, regardless of which layer enforces the limit.
        let mut json = String::new();
        for _ in 0..500 {
            json.push('[');
        }
        json.push('1');
        for _ in 0..500 {
            json.push(']');
        }
        let doc = format!("```json\n{json}\n```");
        let spec = min_spec();
        let issues = structured_data_issues(&spec, &doc);
        assert!(
            issues.iter().any(|i| i.contains("Failed to parse")),
            "{:?}",
            issues
        );
    }

    #[test]
    fn truncated_jsonld_reports_syntax_error_not_panic() {
        let doc = "```json\n{\"@context\":\"https://schema.org\",\"@type\":\"Article\"\n```";
        let spec = min_spec();
        let issues = structured_data_issues(&spec, doc);
        assert!(
            issues.iter().any(|i| i.contains("Failed to parse")),
            "{:?}",
            issues
        );
    }

    #[test]
    fn jsonld_with_scalar_root_does_not_panic() {
        // A code fence tagged ```json whose content is a bare scalar (valid JSON, but not
        // an object/array) must not panic collect_types/schema_field_issues.
        let doc = "```json\n42\n```";
        let blocks = extract_jsonld_blocks(doc);
        assert_eq!(blocks.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&blocks[0]).unwrap();
        let mut types = BTreeSet::new();
        collect_types(&v, &mut types);
        assert!(types.is_empty());
        assert!(schema_field_issues(&v).is_empty());
    }

    #[test]
    fn jsonld_with_non_array_main_entity_does_not_panic() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"@context":"https://schema.org","@type":"FAQPage","mainEntity":"not an array"}"#,
        )
        .unwrap();
        let issues = schema_field_issues(&v);
        assert!(
            issues.iter().any(|i| i.contains("mainEntity")),
            "{:?}",
            issues
        );
    }

    #[test]
    fn jsonld_with_wide_flat_array_of_many_types_does_not_panic() {
        // Wide (not deep) structure: many sibling nodes rather than nested ones.
        let items: Vec<String> = (0..5_000)
            .map(|i| format!(r#"{{"@type":"Thing{i}"}}"#))
            .collect();
        let doc = format!("```json\n[{}]\n```", items.join(","));
        let blocks = extract_jsonld_blocks(&doc);
        let v: serde_json::Value = serde_json::from_str(&blocks[0]).unwrap();
        let mut types = BTreeSet::new();
        collect_types(&v, &mut types);
        assert_eq!(types.len(), 5_000);
    }

    // --- Edge cases: extreme unicode -------------------------------------------------

    #[test]
    fn rtl_arabic_text_word_count_and_first_paragraph_do_not_panic() {
        let doc = "# عنوان\n\nهذه هي الفقرة الأولى التي تجيب مباشرة على السؤال الأساسي هنا الآن.\n";
        let para = first_paragraph(doc);
        assert!(!para.is_empty());
        assert!(word_count(&para) > 0);
    }

    #[test]
    fn emoji_zwj_sequences_and_astral_plane_chars_do_not_panic() {
        // Family emoji (ZWJ sequence), flag emoji (regional indicator pair), and an
        // astral-plane mathematical bold character — all multi-byte, some multi-codepoint
        // grapheme clusters.
        let doc = "# 👨‍👩‍👧‍👦 Title 🇰🇷\n\n\
                    Real opening paragraph with 𝕳𝖊𝖑𝖑𝖔 text and enough words to pass here.\n\n\
                    ## FAQ\n\nQ: 질문 emoji 👍?\nA: 답변 emoji 👌.\n";
        let m = metrics(doc);
        assert_eq!(m.faq_qa_count, 1);
        let para = first_paragraph(doc);
        assert!(para.contains("𝕳𝖊𝖑𝖑𝖔"));
        // truncate() must not panic mid-grapheme on emoji/ZWJ input.
        let _ = crate::llm::truncate(doc, 5);
        let _ = crate::llm::truncate(doc, doc.chars().count() + 100);
    }

    #[test]
    fn combining_diacritics_do_not_panic() {
        // "e" + combining acute accent (U+0301), rather than the precomposed "é".
        let doc = "# Cafe\u{0301} Title\n\nThis is the real opening paragraph, cafe\u{0301} style, long enough now.\n";
        let para = first_paragraph(doc);
        assert!(!para.is_empty());
        let heads = parse_headings(doc);
        assert_eq!(heads.len(), 1);
    }

    #[test]
    fn crlf_line_endings_are_handled_like_lf() {
        let doc = "# Title\r\n\r\nReal direct answer paragraph with enough words to pass the check here now.\r\n\r\n```\r\n# fenced heading, ignored\r\n```\r\n";
        let heads = parse_headings(&strip_code_fences(doc));
        assert_eq!(heads.len(), 1, "{:?}", heads);
        assert_eq!(heads[0].text, "Title");
    }
}
