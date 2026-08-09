use crate::llm::Llm;
use crate::schema;
use crate::spec::Spec;
use anyhow::Result;

pub const SYSTEM: &str = "You are a GEO (Generative Engine Optimization) content expert. \
You write in a form that generative answer engines like ChatGPT/Perplexity/Google AI Overview can directly quote and excerpt. \
You prioritize verifiable statistics, sources, and clear definitions over exaggerated rhetoric. \
Never fabricate unverified figures, and always attach a source via a markdown link for every statistic.";

fn structure_requirements(spec: &Spec) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "- The document starts with a single `# Title` (H1).\n\
         - In the first paragraph right after the H1 (within {max_words} words including spaces), give a \
         direct, complete-sentence answer to the document's core question. This paragraph alone should be \
         enough to understand the answer.\n\
         - Include at least {min_stat} statistics/figures throughout the document, and cite the source for \
         each one nearby as a markdown link `[source name](URL)`.\n\
         - Under an 'FAQ' heading (##), include at least {min_qa} question-answer pairs, each written as a \
         `Q: question` line followed by an `A: answer` line (bold formatting is fine, but each line must \
         start with Q:/A:).\n\
         - Respect the heading hierarchy (H1 → H2 → H3, don't skip levels).\n\
         - At the very end of the document, write schema.org JSON-LD in a ```json code block. \
         The `@type` must include the following: {types}. If both Article and FAQPage are required, \
         either combine them in one block using a `@graph` array, or split into two separate ```json \
         blocks. For FAQPage, map the `mainEntity` array's `Question` (name) and `acceptedAnswer` \
         (`Answer`.text) 1:1 to the FAQ questions and answers in the body.\n",
        max_words = spec.answer_summary.max_words,
        min_stat = spec.statistics.min_count,
        min_qa = spec.faq.min_qa,
        types = spec.structured_data.required_types.join(", "),
    ));
    if spec.llms_txt.enabled {
        p.push_str(
            "- Additionally, at the very end of the document (after the JSON-LD block), write an llms.txt \
             snippet in a ```llms.txt code block. The first line is `# document/site name` (H1), the next \
             line is blank, then `> one-line summary` (blockquote), followed by a markdown list of related \
             links `- [label](URL)` (at least 20 words total).\n",
        );
    }

    let types_lower: Vec<String> = spec
        .structured_data
        .required_types
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    if types_lower.contains(&"article".to_string()) && types_lower.contains(&"faqpage".to_string())
    {
        let example = schema::example_graph(
            &schema::ArticleMeta {
                headline: "<document title>",
                description: "<one-sentence summary>",
            },
            &[schema::QaPair {
                question: "<example FAQ question>".to_string(),
                answer: "<example FAQ answer>".to_string(),
            }],
        );
        p.push_str(&format!(
            "\nExample JSON-LD shape (fill in values with actual document content, use only as a structural reference):\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&example).unwrap_or_default()
        ));
    }
    p
}

/// Prompt for the initial generation.
pub fn build_prompt(spec: &Spec, idea: &str, angle: &str) -> String {
    let mut p = String::new();
    p.push_str(
        "# Task\nWrite a GEO-optimized draft document that meets the requirements below.\n\n",
    );
    p.push_str(&format!("## Document: {}\n{}\n\n", spec.name, spec.context));
    if !angle.is_empty() {
        p.push_str(&format!(
            "## Differentiation angle for this draft\n{}\n\n",
            angle
        ));
    }
    p.push_str(&format!(
        "## Source material (topic/evidence)\n{}\n\n",
        idea
    ));
    p.push_str(&format!(
        "## Body section structure\n{}\n\n",
        spec.sections_prompt()
    ));
    p.push_str(&format!(
        "## Evaluation criteria (keep these in mind while writing)\n{}\n\n",
        spec.rubric_prompt()
    ));
    if spec.total_words > 0 {
        p.push_str(&format!(
            "## Total length\nApprox. {} words\n\n",
            spec.total_words
        ));
    }
    p.push_str("## GEO structure requirements (all mandatory)\n");
    p.push_str(&structure_requirements(spec));
    p.push('\n');
    p.push_str(
        "## Output rules\n\
         - Output in markdown. Output only the document body — no introduction, explanation, or meta-commentary.\n\
         - If a fact is uncertain, don't make it up; mark it as an 'estimate' or use a verifiable expression.\n\
         - Use markdown tables wherever a table is effective (comparisons, specs, steps).\n",
    );
    p
}

/// Prompt for regenerating a document based on grading feedback.
pub fn build_revise_prompt(
    spec: &Spec,
    idea: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> String {
    let mut p = String::new();
    p.push_str("# Task\nRevise the GEO document draft below according to the review feedback and output the entire document again.\n\n");
    p.push_str(&format!("## Document: {}\n{}\n\n", spec.name, spec.context));
    p.push_str(&format!(
        "## Source material (topic/evidence)\n{}\n\n",
        idea
    ));
    p.push_str(&format!("## Current draft\n{}\n\n", prev_doc));
    p.push_str(&format!(
        "## Review feedback (must be addressed)\n{}\n\n",
        feedback
    ));
    if !weak.is_empty() {
        p.push_str(&format!(
            "## Items with especially low scores\n{}\n\n",
            weak
        ));
    }
    p.push_str(&format!(
        "## Evaluation criteria\n{}\n\n",
        spec.rubric_prompt()
    ));
    p.push_str("## GEO structure requirements (all mandatory, keep as is)\n");
    p.push_str(&structure_requirements(spec));
    p.push('\n');
    p.push_str(
        "## Output rules\n\
         - Output the entire improved document in markdown. No change summary or meta-commentary.\n\
         - Keep the parts that are already well-written, and substantively strengthen only the parts that were flagged.\n\
         - Don't fabricate new figures without evidence. If you can't support a claim, remove it or tone down the wording.\n\
         - Don't respond by simply making the document longer. Keep the total length within ±15% of the current draft, \
         and improve it by replacing weak sentences.\n",
    );
    p
}

pub fn generate(llm: &Llm, spec: &Spec, idea: &str, angle: &str) -> Result<String> {
    let prompt = build_prompt(spec, idea, angle);
    llm.text(&prompt, Some(SYSTEM))
}

pub fn revise(
    llm: &Llm,
    spec: &Spec,
    idea: &str,
    prev_doc: &str,
    feedback: &str,
    weak: &str,
) -> Result<String> {
    let prompt = build_revise_prompt(spec, idea, prev_doc, feedback, weak);
    llm.text(&prompt, Some(SYSTEM))
}

/// If there aren't enough angles, fill in with default angles and return n of them.
pub fn angles_for(spec: &Spec, n: usize) -> Vec<String> {
    let defaults = [
        "Puts product/service comparisons and decision criteria front and center.",
        "Puts step-by-step how-to instructions front and center.",
        "Puts definitions, concept explanations, and clarification of key entities front and center.",
        "Puts industry statistics and trend data front and center.",
        "Directly addresses common misconceptions and objections via FAQ.",
        "Puts differentiation from competing alternatives and category leadership front and center.",
    ];
    let pool: Vec<String> = if spec.angles.is_empty() {
        defaults.iter().map(|s| s.to_string()).collect()
    } else {
        spec.angles.clone()
    };
    (0..n).map(|i| pool[i % pool.len()].clone()).collect()
}
