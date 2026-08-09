//! schema.org JSON-LD builder.
//!
//! Ported from: [ai-search-guru/getcito](https://github.com/ai-search-guru/getcito-worlds-first-open-source-aio-aeo-or-geo-tool)
//! (MIT). Rewritten in Rust from the field structure of `faqJsonLd()` / `articleJsonLd()`
//! in `apps/www/src/lib/seo.ts`. Rather than porting the original TypeScript
//! (`jsonLd()` wrapper + `@context`/`@type` composition) as-is, this was rewritten as
//! functions that build a `serde_json::Value` tailored to the fields this project
//! deals with (headline, description, question-answer pairs).
//!
//! Used to build the "correct-answer shape" example shown to the model in the
//! generation prompt (`generate.rs`). Actual scoring is done by `checks.rs`, which
//! parses the JSON-LD the model outputs and validates `@type`.

use serde_json::{json, Value};

pub struct ArticleMeta<'a> {
    pub headline: &'a str,
    pub description: &'a str,
}

pub struct QaPair {
    pub question: String,
    pub answer: String,
}

/// FAQPage(@type) JSON-LD. The Question/acceptedAnswer structure of the `mainEntity`
/// array has the same field shape as getcito's `seo.ts` `faqJsonLd()`.
pub fn faq_jsonld(items: &[QaPair]) -> Value {
    let main_entity: Vec<Value> = items
        .iter()
        .map(|qa| {
            json!({
                "@type": "Question",
                "name": qa.question,
                "acceptedAnswer": {
                    "@type": "Answer",
                    "text": qa.answer,
                }
            })
        })
        .collect();
    json!({
        "@type": "FAQPage",
        "mainEntity": main_entity,
    })
}

/// Article(@type) JSON-LD core fields.
pub fn article_jsonld(meta: &ArticleMeta) -> Value {
    json!({
        "@type": "Article",
        "headline": meta.headline,
        "description": meta.description,
    })
}

/// Example combining Article + FAQPage into a `@graph` (scaffold shown in the prompt).
pub fn example_graph(article: &ArticleMeta, qa_sample: &[QaPair]) -> Value {
    json!({
        "@context": "https://schema.org",
        "@graph": [
            article_jsonld(article),
            faq_jsonld(qa_sample),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks;

    #[test]
    fn example_graph_has_required_types() {
        let g = example_graph(
            &ArticleMeta {
                headline: "Title",
                description: "Summary",
            },
            &[QaPair {
                question: "Question?".to_string(),
                answer: "Answer".to_string(),
            }],
        );
        let mut types = std::collections::BTreeSet::new();
        checks::collect_types(&g, &mut types);
        assert!(types.contains("Article"));
        assert!(types.contains("FAQPage"));
    }
}
