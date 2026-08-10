# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08-10

Initial release.

### Added

- `geo gen`: generate N angle-varied drafts from a spec + idea file, score them, and write
  a ranked report (`src/main.rs`, `src/generate.rs`).
- `geo score`: score existing `.md`/`.txt` documents against a spec without generating.
- `geo loop`: generate → deterministically check → LLM-judge score → regenerate
  self-improvement loop, with target-score early stop, stagnation detection, an optional
  held-out `--gate-model` re-score, and a "spike after stagnation" reward-hacking warning
  (`src/loop_run.rs`).
- `geo probe`: brand-blind cross-check that re-asks a document's FAQ questions to a
  sub-agent with no document/brand context and reports deterministic keyword/number
  overlap against the document's own answers (`src/probe.rs`).
- Deterministic structural checks (`src/checks.rs`), independent of LLM judgment: direct-answer
  opening paragraph, sourced-statistics count, FAQ `Q:`/`A:` pairs, JSON-LD structured data
  (`@type` presence + Google-documented required/recommended field checks for
  `FAQPage`/`Article`/`Product`/`HowTo`), heading-hierarchy consistency, and an optional
  `llms.txt` snippet validator (ported from Auriti-Labs/geo-optimizer-skill's
  `_validate_llms_content`).
- LLM rubric judge (`src/score.rs`): multi-round, multi-model judge panel with trimmed-mean
  aggregation, spec-driven weighted scoring, and a self-solve-first judging protocol
  (`winning_conditions` + `ideal_sketch` written before the model reads the document).
- `claude -p` subprocess backend (`src/llm.rs`): `--safe-mode`, `--tools ""`, JSON Schema
  structured output, per-call timeout/retry, and cumulative cost tracking.
- Spec file format (`specs/*.toml`, `src/spec.rs`) with validation (non-empty criteria,
  positive weights, unique criterion IDs, non-empty required structured-data types).
- Recursion-depth guard (`MAX_JSON_DEPTH`, `src/checks.rs`) on JSON-LD `@type`
  collection/field-checking, defending the traversal against abnormally deep JSON-LD
  emitted by a low-trust LLM output source.

### Fixed

*(all four found and fixed via static code review before any real API cost was spent —
see [`evals/README.md`](evals/README.md) for the full write-up)*

- `extract_faq_pairs`/`faq_metrics` left a stray `**` in extracted FAQ text for bold-labeled
  pairs like `**Q:** ...` (only whole-line-wrapping `**` was stripped, not `**` wrapping just
  the label) ([#2](https://github.com/Loop-Suite/GEO-Loop/issues/2), `6035491`).
- `checks::first_paragraph` (used by the answer-summary check) scanned the raw, un-stripped
  document, so a `#`-comment line inside a fenced code example could be misread as the
  document's H1, silently discarding the real opening paragraph
  ([#4](https://github.com/Loop-Suite/GEO-Loop/issues/4), `1a39dfa`).
- `checks::norm()` (heading/spec-title matching in `missing_sections()`) only stripped
  whitespace, not case, so a heading differing from the spec title only in case (`## overview`
  vs. `"Overview"`) was falsely flagged as missing
  ([#5](https://github.com/Loop-Suite/GEO-Loop/issues/5), `7c13429`).
- `geo probe` called `checks::extract_faq_pairs` directly on the raw document (unlike `geo
  score`'s FAQ count, which pre-strips fences), so a fenced example demonstrating the FAQ
  format could be extracted and sent to the brand-blind probe call as if it were a real FAQ
  entry ([#6](https://github.com/Loop-Suite/GEO-Loop/issues/6), `061f9de`).

### Security

- Judge-reported criterion scores are no longer trusted at face value: `score::effective_score`
  independently caps a criterion's score at 60 whenever its self-reported evidence quote is
  under 30 characters (trimmed), matching the cap documented in this README's scoring
  pipeline but previously enforced nowhere server-side — a judge model could otherwise return
  a high score backed by empty/trivial evidence and have it accepted as-is
  ([#3](https://github.com/Loop-Suite/GEO-Loop/issues/3), `aa4b842`).
- Re-audited `checks.rs`'s JSON-LD parsing path for resource exhaustion via oversized/deeply
  nested input: confirmed the pinned `serde_json` 1.0.151 already enforces a ~128-level
  recursion limit on both array and object nesting and returns `Err` rather than overflowing
  the stack, ahead of and independent of this project's own `MAX_JSON_DEPTH` traversal guard.
  No code change was required; added regression tests
  (`jsonld_exceeding_serde_recursion_limit_reports_parse_error_not_panic` and neighbors in
  `src/checks.rs`) pinning down that a document engineered to trip this returns a reported
  format issue, not a crash.
- Re-audited every call site of every fence-stripping-dependent function in `checks.rs`
  (`parse_headings`, `split_sections`, `stat_token_count`, `source_link_count`,
  `faq_metrics`, `first_paragraph`, `heading_hierarchy_issues`, `metrics`,
  `missing_sections`, `extract_faq_pairs`, `extract_jsonld_blocks`) for the guard-gap bug
  class behind #4/#6 above. All current call sites are consistent (each function either
  strips fences internally or its only caller pre-strips) — no further instance found.
- Reviewed all file-path handling (`main.rs`, `loop_run.rs`, `probe.rs`, `report.rs`,
  `spec.rs`) for path traversal: every path is either a CLI argument supplied directly by
  the operator or a deterministic label the program generates itself (e.g. `iter01.md`,
  `cand01.md`); none is derived from untrusted document/spec content, so there is no
  meaningful trust boundary for traversal to cross in this local, single-user CLI.

[0.1.0]: https://github.com/Loop-Suite/GEO-Loop/releases/tag/v0.1.0
