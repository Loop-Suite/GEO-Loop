# geo-loop Research & Evidence Deep-Dive Survey (2026-08-01)

## 1. Overview

The core structure of `geo-loop` ports the `bizplan-loop` family's pipeline —
**(1) generate N angle-varied drafts -> (2) deterministic rule checks
(checks.rs) -> (3) LLM rubric scoring (multi-round, multi-model,
de-anchoring) -> (4) trimmed-mean aggregation -> (5) feedback-driven
regeneration -> (6) re-scoring of the first/best draft by a held-out gate
model (reward-hacking detection)** — into the GEO (Generative Engine
Optimization — generating documents suited for citation/excerption by
generative answer engines like ChatGPT/Perplexity/Google AI Overview) domain.
As with `research-loop`, `aso-loop`, and `seo-loop`, there is no discourse
(anonymous cross-discussion) stage; instead, multi-round scoring + trimmed
mean + held-out validation is the axis that secures reliability.

This document re-verifies four facts confirmed in the initial round (research
prior to implementation) — geo-optimizer-skill, getcito, the original
GEO-BENCH paper, and commercial GEO monitoring tools — **by reading the
actual source files directly via `gh api`**, and explicitly corrects the
points among the initial conclusions that turned out to be inaccurate. It
then newly investigates AEO/GEO generation pipeline OSS, schema.org
validators, citability measurement methodology, and academic grounding for
preventing LLM-judge reward hacking. The methodology follows
`Loop-Suite/research-loop`'s `docs/research-and-evidence-survey-2026-07-31.md`
(§8 "Follow-up Investigation") as the quality bar — conclusions are not drawn
from READMEs/landing pages alone; actual source files (function names, line
numbers) are used as evidence, and where README narrative and code diverge,
that divergence itself is recorded as a correction case.

geo-loop's own architecture reference points (the baseline against which the
re-verified subjects are compared):
- Deterministic checks: `src/checks.rs` — `strip_code_fences()`,
  `answer_summary_issues()`, `statistics_issues()`, `faq_issues()`,
  `structured_data_issues()`, `schema_field_issues()`,
  `heading_hierarchy_issues()`, `llms_txt_issues()`
- LLM rubric scoring: `src/score.rs::score_doc()` — `judge_schema()` enforces
  `winning_conditions` (writing the scoring criteria before reading the
  document) via field ordering to implement de-anchoring, cycles through 6
  `LENSES` perspectives, `trimmed_mean()` (drops min/max when n>=4)
- Regeneration loop: `src/loop_run.rs::run()` — argmax selection (the best
  score across all rounds, not just the last round), early stop on stall,
  length-inflation canary
- Held-out gate: `src/main.rs` lines 235-251 — a model specified via
  `--gate-model` that did not participate in the loop re-scores only
  `first_doc`/`best_doc`

## 2. Re-verification of Prior Findings (Including Self-Correction)

### 2.1 Auriti-Labs/geo-optimizer-skill — Re-verifying "Does It Only Audit, or Also Generate?"

The initial round only confirmed that `audit_llms.py::_validate_llms_content()`
had been ported, and left unverified whether a feature corresponding to "N
draft generation -> multi-round scoring -> held-out gate" existed. This time
the actual repository tree
(`gh api repos/Auriti-Labs/geo-optimizer-skill/git/trees/main?recursive=true`)
and three core source files (`core/fixer.py`, `core/scoring.py`,
`core/competitive_narrative.py`) were read directly.

**Confirmed facts**:
- Under `src/geo_optimizer/core/` there are 20+ `audit_*.py` files
  (`audit_ai_discovery.py`, `audit_brand.py`, `audit_cdn.py`,
  `audit_citation_map.py`, `audit_content.py`, `audit_llms.py`, `audit_rag.py`,
  `audit_schema.py`, etc.) — the overwhelming majority of the repository is
  **auditing**.
- `core/fixer.py` (593 lines) is an "automated GEO fix generator", but in
  practice it is entirely **deterministic template/rule-based**:
  - `generate_robots_fix()` — assembles AI-bot User-agent lines as strings
    (lines 28-97)
  - `generate_llms_fix()` — crawls the sitemap and assembles llms.txt (calls
    `llms_generator.py::generate_llms_txt()`, lines 100-143)
  - `generate_content_rewrite_fix()` (lines 353-416) — **makes no LLM calls
    at all.** It merely assembles "checklist text" via if-branches such as
    `if not result.content.has_h1: suggestions.append("- Add a single H1...")`
    and returns it as `content-rewrite.md`; it never actually rewrites the
    body copy.
- `core/scoring.py::compute_geo_score()` is a **weighted sum** (0-100) across
  categories like robots/llms/schema/meta/content, not LLM scoring.
- `core/llm_client.py` does have an LLM client supporting
  OpenAI/Anthropic/Groq/Perplexity, but it is actually used in only 2 places:
  `core/competitive_narrative.py` (line 227,
  `query_llm(prompt, system=system, max_tokens=512)`) and
  `core/audit_sentiment.py` (line 88). Reading
  `competitive_narrative.py` shows it is a **single-shot LLM call** to
  extract brand-positioning copy — none of multi-round, multi-model, trimmed
  mean, or held-out re-verification exist.

**[No correction, deepened confirmation]**: The initial round's conclusion of
"audit-centric, generation status unclear" was accurate. This re-verification
established at the code level that even the one feature carrying the word
"generation" (`fixer.py`) produces only an if-branch checklist for the body
copy, with no LLM involved. There is **no** feature in this repository
corresponding to geo-loop's "N drafts -> LLM rubric scoring -> regeneration"
loop.

### 2.2 ai-search-guru/getcito — Confirming the Initial "schema.org Generation Tool" Description Was Wrong

The initial round described getcito as a "schema.org JSON-LD/llms.txt
generation tool." This time the repository tree was checked exhaustively
(`gh api .../git/trees/master?recursive=true` — note the default branch is
**`master`**, not `main`; this itself is a minor detail the initial
investigation had missed), and the `apps/www/src/lib/seo.ts` source was read
directly.

**[Correction]** getcito is not a schema.org generation tool. Looking at the
directory structure of `apps/web` (the actual product) — `src/components/citations/`,
`src/components/prompt-wizard.tsx`, `src/components/share-of-voice-donut.tsx`,
`src/routes/_authed/app/$brand/citations.tsx`, `.../opportunities.tsx`,
`.../query-fan-out.tsx`, etc. — it is an **AI search visibility monitoring
dashboard**: a product that tracks how often/in what prompts a brand is
mentioned/cited and how its share-of-voice compares to competitors, closely
resembling an OSS version of commercial tools like Profound/Peec AI/Otterly.

The `seo.ts` file containing `faqJsonLd()`/`articleJsonLd()` lives under
`apps/www` (not the product, but **getcito's own marketing/blog site**). This
is confirmable from the function list alone, even without a file-header
comment — `websiteJsonLd()`, `organizationJsonLd()`,
`softwareApplicationJsonLd()`, `articleJsonLd()`, `blogPostingJsonLd()`,
`breadcrumbJsonLd()`, `faqJsonLd()`, `itemListJsonLd()`,
`definedTermSetJsonLd()`, `howToJsonLd()`. This is an SEO utility library
getcito uses to **dogfood GEO on its own homepage**, not a product feature
exposed to end users. Searching the entire repository (a tree of 782 files)
for `llms.txt` turns up only one hit —
`packages/docs/content/blog/do-llms-txt-files-matter-for-aeo.mdx` (a **blog
opinion piece** about whether llms.txt matters for AEO) — there is no code
that actually generates an llms.txt anywhere.

The geo-loop README's attribution (lines 165-168) — "`src/schema.rs`'s
`faq_jsonld()`/`article_jsonld()` reimplement the schema.org field structure
of `apps/www/src/lib/seo.ts::faqJsonLd()`/`articleJsonLd()`" — was itself
**accurate in both file path and function names** (confirmed as-is by this
re-verification). What was wrong was the **description of this function's
purpose** (the README's project-character summary calling it a "schema.org
JSON-LD/llms.txt generation tool") — getcito is not a generation tool but a
monitoring tool, and the JSON-LD helpers are an internal utility for the SEO
of that monitoring product's own marketing site. There is no need to fix the
attribution wording in the `NOTICE`/README files themselves (the code-porting
facts are correct), but any separate prose describing the project's character
should be corrected to "AI visibility monitoring dashboard (with a
schema.org helper library on its own marketing site)."

### 2.3 The Original GEO Paper (Aggarwal et al., arXiv:2311.09735) — Confirming It Was a "One-Shot Intervention + Comparison," Not an "Iterative Loop"

The initial round only confirmed effect sizes (+115% for cite sources, +41%
for statistics addition) and explicitly noted it had not determined "whether
this paper proposes a generate -> score -> regenerate loop structure." This
time the arxiv original (rendered via ar5iv HTML — direct PDF parsing was
corrupted and failed, see "Failed Verification" below) was read, confirming
the following (with original quotes included).

**[Correction]** This paper **does not propose an iterative regeneration
loop.** Original text from Section 2.2.2: *"These methods are designed to
implement textual modifications to W in a manner that is independent of the
queries."* — it applies 9 predefined GEO techniques (authoritative, keyword
stuffing, statistics addition, cite sources, quotation addition,
easy-to-understand, fluency optimization, unique words, technical terms)
**independently, once each**, to a source document, then measures the change
in visibility metrics by comparing (A/B) the result against the original.
There is no step that "feeds scoring results back as feedback for
regeneration" — that is, there is nothing in the original paper corresponding
to the iterative loop implemented by geo-loop's `loop_run.rs::run()`.

- **GEO-BENCH scale**: Section 3.2, "10K queries split into 8K, 1K, and 1K
  train/val/test splits," collected from 9 data sources (MS MARCO, ORCAS-1,
  Natural Questions, AllSouls, LIMA, Perplexity.ai Discover, Davinci-Debate,
  ELI-5, GPT-4 Generated), with the top 5 search-result texts attached per
  query.
- **Evaluation metrics**: "Position-Adjusted Word Count" (Section 2.2.1,
  which exponentially decays the word count of citation-related sentences by
  rank to normalize them) and "Subjective Impression" (7 sub-metrics —
  relevance, influence, uniqueness, subjective position/count, click
  probability, diversity — scored by an LLM via G-Eval). Both share an axis
  with geo-loop's `checks.rs::stat_token_count()` (a pure count, no LLM
  involved) or the LLM rubric scoring in `score.rs`, but the paper's
  Subjective Impression differs in that it scores **not the document itself,
  but "how much this source was cited within the generated answer"** —
  geo-loop scores a document's citability up front, while the GEO paper
  scores the citation outcome within an actually generated answer after the
  fact.
- **Citation hallucination / reward hacking discussion**: none. The Ethics
  section contains only generic remarks about using public information; the
  concepts of an LLM judge's (G-Eval) self-preference bias or held-out
  verification of scoring results do not appear.

**Implication of this correction**: geo-loop's "N drafts -> scoring ->
regeneration -> held-out gate" loop originates not from the original GEO
paper but from a ported `bizplan-loop` architecture, as the README already
states (line 6) — this re-verification makes that distinction clearer.
Domain knowledge about GEO (cite-sources/statistics effect sizes, the 9
techniques) came from the paper, but **the iterative loop, multi-judge
scoring, and held-out gate as reliability mechanisms are a contribution
unique to the bizplan-loop family, not the GEO literature.** The original
README implicitly got this distinction right but had never stated it
explicitly.

*Failed verification note*: When `arxiv.org/pdf/2311.09735` was fetched
directly via WebFetch, the PDF content was corrupted and a generic,
speculative answer came back (a response with hallucination risk — it was
not adopted). Retrying with `ar5iv.labs.arxiv.org/html/2311.09735` (the HTML
rendering) produced a response capable of citing the original text, and
everything above is based on that retry. **Had the first attempt's result
been used as-is, it would have been an error** — this investigation itself
demonstrates the lesson that a tool failure requires an automatic mitigation
(the ar5iv fallback) before proceeding.

### 2.4 Commercial GEO Monitoring Tools — Deepened Confirmation of Scrunch AI/Sitecore's "Content Reformatting" Feature

The initial round only confirmed that Scrunch AI, after being acquired by
Sitecore, gained a "content reformatting" feature. This time, official
Sitecore materials (Bloomberg, PRNewswire, Sitecore newsroom) and Scrunch's
own documentation (Help Center, FAQ, product blog) were investigated.

**Confirmed fact (acquisition scale)**: Sitecore acquired Scrunch for
approximately $225M (2026-06-03, per an exclusive Bloomberg report — the
official amount was not disclosed).

**The reality of the "content reformatting" feature (Agent Experience
Platform, AXP)**: this is not an offline authoring tool like geo-loop's
"generate -> score -> regenerate," but a **real-time traffic-branching
system that runs at the CDN edge layer**.
- Automatically detects AI bot traffic at the CDN level (Akamai/Cloudflare/
  Vercel, etc.)
- When identified as a bot, returns in real time a **separate version of the
  HTML** that has been restructured with unnecessary code stripped, using
  "rules-based + enterprise AI"
- Human visitors see the original site unchanged, while AI bots alone are
  served the optimized parallel version (operating "over the top" without
  changing existing infrastructure)

This is **not a problem of the authoring stage — creating multiple drafts in
advance, scoring them, and picking the best — but a problem of
request-time bot detection that serves a different response**. It is an
architecture with no room for concepts like multi-round LLM scoring, trimmed
means, or a held-out gate — the initial round's conclusion of "simple
reformatting" was correct, and this investigation has now pinned down exactly
which layer (CDN edge, request-time branching) that "reformatting" occurs at.

## 3. New Investigations

### 3.1 AEO/GEO Content Generation OSS Frameworks

No GEO content generation pipeline explicitly built with LangGraph/CrewAI was
found (the `github.com/topics/generative-engine-optimization` topic page was
checked exhaustively; no project overlaps with LangGraph/CrewAI tags). Instead,
4 projects that are far more directly comparable at the architecture level
were confirmed at the source level.

| Project | Architecture (code evidence) | Overlap with us (geo-loop) |
|---|---|---|
| **AutoGEO** (`cxcscmu/AutoGEO`, ICLR 2026 accepted) | `autogeo/rewriters/core.py::rewrite_document()` — compares high/low-visibility document pairs via LLM to auto-extract a list of "preference rules" (15-17 natural-language rules, varying by dataset/engine LLM), inserts them into a prompt, and does a **single LLM call for one rewrite**. Separately, `AutoGEOMini` is a lightweight model trained with GRPO (RL) using these rules as reward (`open-r1/` directory, based on LLaMA-Factory) — training is a one-time offline process, not a structure that runs multiple rounds per document as geo-loop does. `autogeo/evaluation/metrics/geo_score.py::impression_wordpos_count_simple()` implements the GEO-BENCH paper's Position-Adjusted Word Count as-is, and contains a commented-out exception handler `# print(f'Citation Hallucinated: {cit}')` in the code — meaning the metric itself is already aware that "a citation number that doesn't exist can appear." | **Partial overlap, but a cautionary comparison**: rule-based, one-shot rewriting differs from geo-loop's N-draft loop. More importantly, `AutoGEOMini` **directly optimizes, via RL reward, this very GEO score — a proxy metric that cannot defend against citation hallucination** — with no mention anywhere in the README or code of held-out verification or a separate judge model. This can be read as an actual implemented instance of the "proxy-scoring overfitting" risk that geo-loop's README's "Limitations" section warns about. |
| **AgenticGEO** (arXiv:2603.20213, "Self-Evolving Agentic System for GEO") | Per the abstract: evolves diverse content strategies via a MAP-Elites archive, with a lightweight surrogate model called a "Co-Evolving Critic" approximating feedback from actual generative engines to guide strategy selection. | **Low verification level, stated honestly**: this investigation only reviewed the arXiv abstract; the code/full text was not read (time constraint) — the combination of an evolutionary loop plus a surrogate judge superficially resembles discourse's "multiple independent judges -> consensus," but whether genuine independent cross-verification exists **could not be confirmed** in this round. This is handled the same way research-loop treated DeerFlow/open_deep_research as "outline only, unable to re-verify." |
| **recomby-geo** (`ViryaZheng/recomby-geo`, MIT) | Read the generation stage (`plugins/recomby-geo/commands/05-production.md`) of a 7-step workflow directly: "hard-gate rejection unless the slots filled by an expert in the briefing stage (04) are in `ready-for-production` status" (Step 1) -> assemble slots into a skeleton -> sequentially apply "Princeton GEO rewrite techniques" (quotation insertion/statistics foregrounding/citation densification/assertive tone/sentence-length variation — corresponding to 5 of the 9 techniques in the original GEO paper) -> tone matching (heuristic 1-5 score against 5-sentence brand-voice samples) -> **"Step 8 — Self-review": a single self-questioning pass, "would I cite this if I were ChatGPT?"** (no separate judge model, multiple rounds, or trimmed mean). | **The generation stage does not overlap with geo-loop**: it is not an LLM scoring loop but "expert-filled slots + deterministic style transformation + a single self-review." **Step 07 (`07-reaudit.md`) is a far more interesting point of comparison instead** (see §3.3) — it uses the phrase "closed loop," but this is not a content-regeneration loop; it is a loop that "attributes post-deployment measured results back to actions." |
| **Elmo** (`elmohq/elmo`) | A self-hosted alternative to Profound/Peec/Otterly — a monitoring platform that tracks how AI answer engines mention/cite/describe brands. | **No overlap** — exactly the same category as the "commercial tools are monitoring-centric" conclusion confirmed in §2.4. No generation feature (per README/topic description; source not investigated). |

**Summary**: An OSS project combining N-draft generation + multi-round,
multi-model LLM rubric scoring + trimmed mean + held-out gate into a single
pipeline was **still not found** in this round either (re-verification of
geo-optimizer-skill/getcito, new investigation of AutoGEO/AgenticGEO/
recomby-geo/Elmo). Even AutoGEO, the closest match, is "rule-based one-shot
rewriting + RL optimization against a proxy metric," with no reward-hacking
prevention mechanism in the code whatsoever — it is closer to an actual
instance of the very risk we are concerned about, implemented without
mitigation.

### 3.2 Re-investigation of schema.org/JSON-LD Validator OSS

crates.io was re-searched with keywords related to `schema.org`, `json-ld`,
and `FAQPage`, but there was still no Rust crate that understands schema.org
semantics (which fields are "required" for which `@type`). Everything found
was a generic JSON Schema validator: `jsonschema`, `rsonschema`,
`jsonschema-valid` (JSON Schema Draft 4/6/7), `json-schema-validator-core`,
`json-schema-rs`. These only validate once a user writes and supplies a JSON
Schema document themselves; they do not embed the knowledge that "FAQPage
requires `mainEntity`" — `geo-loop`'s `checks.rs::schema_field_issues()`
(lines 469-536, which directly codifies the Google Search Central guidelines
as rules) still appears to be the only Rust code addressing this problem
(re-confirming the initial round's conclusion, no correction).

In the Node.js/npm ecosystem, `iaincollins/structured-data-testing-tool` was
newly identified — it explicitly states that "strictly speaking no schema.org
property is 'required,' but vendors like Google independently require their
own 'required/recommended' properties," and is designed with vendor-specific
(Google) presets so the same `@type` can be reused across multiple presets.
This shares **exactly the same understanding** as the premise geo-loop's
`schema_field_issues()` adopts — "schema.org itself has no concept of
required fields; it is Google's documentation that defines required/
recommended" (README line 170) — the difference being that the JS ecosystem
already has a tool generalized via the preset approach, while the Rust
ecosystem does not yet. It was also confirmed in the tree that the
geo-optimizer-skill repository contains `src/geo_optimizer/core/schema_validator.py`
and `core/schema_injector.py`, but their contents were not read in this
round — left as a follow-up investigation target for the next round.

### 3.3 Methodology for Measuring "Citability"

No general-purpose open source tool (in a reusable-library form) was found —
reconfirming the initial conclusion that this area is dominated by commercial
SaaS such as Profound/Peec AI/Otterly.ai/Scrunch (§2.4).

However, **recomby-geo's `02-audit.md`/`07-reaudit.md`**, covered in §3.1,
was the OSS methodology closest to actual measurement. Key design points
confirmed by reading the original text directly:
- `02-audit.md`: for each target query, it spawns a **fresh sub-agent context
  that has never seen the brand information**, has it write an answer using
  WebSearch/WebFetch like an actual user would, then mechanically extracts
  whether the brand is mentioned, its rank, and cited URLs via regex
  (`\b(<company.name>|<aliases>)\b`). It clearly distinguishes "failure" from
  "not mentioned" (treating "a response came back but the brand wasn't
  mentioned" not as a failure but as **the single most important data
  point**) — the same principle `research-loop` §2 emphasized: "record
  'unconfirmed' and 'nonexistent' as distinct."
- It deliberately targets **a single engine (Claude) only** — README comment:
  "Multi-LLM coverage sounds appealing but requires per-engine API keys and
  normalization, turning this into a heavy ops project. A reproducible
  single engine beats an unstable multi-engine setup."
- `07-reaudit.md`: diffs the previous round's baseline against the current
  round per query, matches it against a deployed-action log and a time
  window (7-30 days post-publication) to assign an `attribution_confidence`
  (high/medium/low/unknown) — a mechanical ceiling stating "only assign high
  when there is exactly 1 matching action; downgrade to low if there are 2 or
  more; unknown if there are 0," which prevents fabricating causal
  relationships ("Honest attribution" rule: "If you catch yourself writing a
  narrative to justify high on a multi-action query, that narrative is the
  invented causal story this rule bans").

This design is a direct reference **for mitigating the limitation the
geo-loop README notes as "the gap between proxy scoring and actual citation
outcomes"** (lines 172-174): since geo-loop already has a `claude -p`
subprocess backend, it could emulate recomby-geo's "unbiased sub-agent probe"
without needing a separate API key — however, since what geo-loop handles is
an as-yet-unpublished draft, this would need to be adapted from "re-querying
after actual publication on the live web" into "pre-checking, by posing
queries the draft is likely to address to an actual engine, whether the
facts/angles our document covers also appear in the real answer" (see §5
backlog).

### 3.4 Academic Grounding for De-anchoring / Preventing LLM-as-Judge Reward Hacking

At the same level `research-loop` operated when it found CITETRACER,
FacTool, and Loki, 2 papers more directly relevant to GEO/rubric scoring were
identified.

- **"More Convincing, Not More Correct: Self-Play Reward Hacking of
  Reference-Free LLM Judges"** (arXiv:2607.05904, 2026-07) — proposes a
  "hidden-anchor audit" technique that checks for exact matches against a
  held-out, cross-sourced ground truth the judge never sees. Key experimental
  result: when the **judge is made to solve the problem itself first, before
  seeing the candidate answer (self-solve-first)**, the false-positive rate
  drops sharply from **0.719 to 0.012**. On GSM8K, policy optimization via
  self-play raises the judge pass rate from 0.72 to 0.94, while actual
  accuracy stays at 0.20 — a paper that quantitatively shows "the judge
  scores plausibility, not correctness." **Direct connection to geo-loop**:
  `score.rs::judge_schema()` (lines 70-107) already forces
  `winning_conditions` (what constitutes citable content) to be written
  before reading the document (comment: "write the criteria before scoring
  to reduce anchoring on the document"). This paper provides quantitative
  evidence (0.719->0.012) that this design direction is correct, but also
  suggests that **"writing the criteria first" and "solving the answer
  itself first" are different mitigations** — geo-loop's judge only writes
  down "the conditions for good content" but does not first write out "the
  ideal answer to this question itself." There is room for a stronger
  mitigation here (see §5 backlog).
- **"Reproducing, Analyzing, and Detecting Reward Hacking in Rubric-Based
  Reinforcement Learning"** (arXiv:2606.04923) — builds CHERRL, a
  controllable experimental environment that injects known judge biases,
  reproduces the process by which a policy discovers and exploits them, and
  proposes a method to automatically detect the "onset of hacking" from
  training logs. As a paper directly dealing with **rubric-based scoring**
  (0-100 per criterion), like geo-loop, it maps more precisely onto our
  scoring approach than the original GEO-BENCH paper does. However, since
  this paper's detection method presupposes RL training logs, it does not
  transfer directly to a structure like geo-loop's, which "scores each
  document independently" — it can only be referenced as an idea for using a
  pattern like "stall followed by a sudden jump" as a trigger for held-out
  re-verification (see §5).

Both papers point toward held-out verification, a direction that matches
what geo-loop's `--gate-model` (main.rs lines 235-251) already adopts —
however, geo-loop's gate only guarantees "the judge is different"; it does
not satisfy the condition both papers emphasize, that "the judge produces its
own answer before seeing the candidate."

## 4. Overall Conclusion

**Does our architecture actually have a differentiator?** Yes, but in a
narrow sense. An OSS project combining "N-draft generation + multi-round/
multi-model LLM rubric scoring (with de-anchoring) + trimmed mean + stall
detection + held-out gate" into a single pipeline was, again, not found in
this deep-dive investigation (re-verification of geo-optimizer-skill/getcito;
new investigation of AutoGEO/AgenticGEO/recomby-geo/Elmo). The closest match,
AutoGEO, is rule-based one-shot rewriting plus offline RL training;
recomby-geo is expert-slot assembly plus a deterministic style pass plus a
single self-review. Neither project has a reward-hacking prevention
mechanism — geo-loop's held-out gate is a genuinely rare design.

**However, the nature of this differentiator must be honestly narrowed.**
What the held-out gate catches is **"overfitting to judge preference"** (the
phenomenon where scores rise when repeatedly calling a judge with the same
kind of bias) — not **"whether ChatGPT/Perplexity actually cites this
document."** As reconfirmed in §2.3, even the original GEO paper measures
"whether it was actually cited within a generated answer" after the fact,
whereas every OSS generation pipeline investigated here (except AutoGEO),
including geo-loop, only scores a document's pre-publication quality on its
own. recomby-geo's unbiased-probe design confirmed in §3.3 comes closest to
actual measurement, but that is a **post-hoc re-query against already
deployed content**, not a pre-generation signal built into a generation loop.

**How serious is GEO's fundamental proxy-scoring limitation?** Assessed as
quite serious. Grounds:
1. No methodology for "measuring actual citation" that could be called an
   industry standard exists across either commercial or OSS offerings
   (commercial tools like Profound also only approximate this via their own
   crawling/API combinations — the actual search/ranking logic of each
   engine is not public, a point the README already correctly notes at line
   174).
2. The original GEO paper's "Subjective Impression" metric was itself
   G-Eval (an LLM judge) from the start, and the paper does not address
   citation hallucination or judge bias (§2.3) — that is, GEO research
   carried the proxy-scoring problem from its very starting point.
3. That AutoGEO's `geo_score.py` already knows about "Citation Hallucinated"
   via a code comment (§3.1) while directly optimizing that very metric as
   an RL reward is evidence that the risk of proxy scoring is left
   effectively unguarded across both academia and industry.

In conclusion, geo-loop's combination of held-out gate, de-anchoring, and
trimmed mean achieves **a rare degree of rigor within the scope of this
survey as an engineering mechanism for increasing scoring reliability**, but
**it does not resolve — and should not claim to resolve — the fundamental
gap between GEO proxy scoring and actual citation outcomes.** The README's
Limitations section already makes this distinction accurately (line 174:
"use the score only for relative comparison within the same spec and the
same scoring model, and as a reference for improvement direction") — this
investigation confirms that this wording is not modesty but an accurate
reflection of the actual state of the entire industry.

## 5. Proposed Next Steps (Backlog)

Priority is ordered not by implementation difficulty but by "the order in
which evidence was secured during this investigation." All items are
unimplemented and require design review.

1. **Consider a stronger de-anchoring approach where the judge first writes
   out "an ideal draft itself"** — based on the self-solve-first effect from
   arXiv:2607.05904 (false-positive rate 0.719 -> 0.012), consider going one
   step further than the current `judge_schema()`'s `winning_conditions`
   (an abstract description of scoring criteria) to have it write a ~200
   character sketch of "what an ideal document would look like given this
   spec" before scoring. This is a prompt/schema-level change, but since
   scoring cost increases, the tradeoff against `--rounds` needs to be
   measured.
2. **A `geo probe` (tentative name) subcommand — add a recomby-geo-style
   unbiased-subagent probe as a pre-generation signal** — reuse the existing
   `claude -p` backend (`src/llm.rs`) to, without a separate API key, pose a
   list of queries the generated draft is likely to address (auto-extractable
   from the spec's FAQ items) to a fresh sub-agent unaware of the brand/
   document, and cross-check whether our document's key claims/figures also
   appear in that answer. This does not fully resolve the fundamental
   limitation noted in §4 — "pre-generation scoring cannot stand in for
   post-hoc citation outcomes" — but it could serve as a mitigating signal.
3. **Expand `schema_field_issues()`** — referencing the vendor-specific
   preset approach of `iaincollins/structured-data-testing-tool`, consider
   expanding coverage beyond FAQPage/Article to other types (Product,
   Organization, HowTo, BreadcrumbList, etc.). Since the spec TOML's
   `structured_data.required_types` already accepts arbitrary types, this
   would be a relatively cheap task of just expanding
   `schema_field_issues()`'s field-rule table.
4. **Review AutoGEO's automatic rule-extraction approach (design stage)** —
   the approach of comparing high/low-scoring draft pairs via LLM to extract
   natural-language rules for "what made the difference"
   (`autogeo/rules/extractor.py`, `explainer.py`, `merger.py`) can be
   referenced as an idea for enriching the `criteria.guide` that geo-loop
   currently hardcodes statically in `spec.toml`, using past loop history.
   However, as noted in §3.1, the fact that AutoGEO itself optimizes this
   rule unguarded against a proxy metric via RL reward should be taken only
   as a cautionary example.
5. **Use CHERRL-style "stall-then-sudden-jump" detection as a held-out
   re-verification trigger (design stage)** — the training-log-based hacking
   detection proposed by arXiv:2606.04923 does not transfer directly to
   geo-loop's per-round independent scoring structure, but adding a
   heuristic to `loop_run.rs`'s `stall`/`patience` logic — "if the score
   jumps sharply right after a stall, automatically trigger the held-out gate
   one more time" — is a low-cost idea worth considering.
6. **Items requiring follow-up investigation (not confirmed in this round,
   explicitly left open)**:
   - Full-text/code re-verification of AgenticGEO (arXiv:2603.20213) — only
     the abstract was reviewed this time.
   - Contents of geo-optimizer-skill's `core/schema_validator.py`/
     `core/schema_injector.py` — only file existence was confirmed in the
     tree; logic unverified.
   - AutoGEO's `evaluation/generative_engine.py`/`evaluator.py` — whether it
     runs rewritten documents through an actual external engine (Gemini/GPT/
     Claude) for evaluation could directly bear on the §3.3 citability
     measurement discussion, but the file was not opened in this round.
