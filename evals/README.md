# Empirical review findings

This documents a two-phase review actually carried out on this repo: (1) a static code
review that filed and fixed 5 issues, and (2) a real CLI execution pass — actual
`claude -p --model haiku --judge-model haiku` subprocess calls, real API cost, not a
simulation — run against an adversarial test document to check whether the 5 fixes hold up
in practice. No bugs were found in phase 2; this is a record of what was checked and what it
cost, not a discovery report.

## TL;DR

| Phase | Scope | Result | Real cost |
|---|---|---|---|
| Round 1 — static review | full repo | 2 issues filed and fixed (#2, #3) | $0 (static) |
| Round 2 — deep-dive on round-1 area | `checks.rs` / `probe.rs` | 3 more issues filed and fixed (#4, #5, #6) | $0 (static) |
| Runtime verification | `geo score` + `geo probe` on a purpose-built adversarial doc | 0 new bugs — all 5 fixes confirmed by measured output | $0.0712 |
| **Total** | | **5/5 issues fixed, 0 found at runtime** | **$0.0712** |

**What this bought:**

- **All 5 bugs were caught by static review, before a single dollar of LLM cost was spent** —
  the cheapest point in the pipeline to catch them. The runtime pass that followed cost
  $0.0712 and, correctly, found nothing new: it was a verification step, not a discovery step.
- **The same root cause showed up independently in two different functions on two different
  call paths.** #4 (`checks::first_paragraph`, used by `geo score`'s answer-summary check) and
  #6 (`checks::extract_faq_pairs`, used by `geo probe`) are the *identical* bug: each function
  skipped the `strip_code_fences()` call that every sibling scanner in `checks.rs` already
  applies, so content inside a fenced code example (a `#`-comment line, a demo `Q:`/`A:` block)
  got read as if it were real document content. Neither was caught by finding the other —
  they were found separately, in the same review pass, because the reviewer went back and
  checked every other caller of `extract_faq_pairs`/functions like it once the first instance
  turned up. **Lesson: once one call site is found missing a shared guard, check every other
  user of that guard — the same class of bug tends to recur wherever the guard was supposed to
  be applied consistently but wasn't.**
- **#5, found in the same pass over the same file, is explicitly *not* the same bug** —
  worth stating precisely rather than folding it into the pattern above just because it was
  found alongside it. Its root cause is `norm()` not lowercasing before a heading-match
  comparison (missing case-normalization), not a missing `strip_code_fences()` call. Three
  issues came out of round 2, but only two of them are one recurring root cause; the third is
  a distinct defect in the same neighborhood.
- **Runtime verification used a single adversarial test document engineered to trip all 5
  fixed bugs at once** — two different fenced-example types (a fake heading + fake stat
  numbers inside a bash example; a fake Q/A block inside a markdown example), 3 bold-labeled
  FAQ pairs (`**Q:**`/`**A:**`), plus real JSON-LD and an `llms.txt` block. Both `geo score`
  (73.8/100) and `geo probe` (3/3 real FAQ pairs extracted, the fenced fake pair ignored) ran
  clean, with measured internal values matching what the fixes were supposed to produce —
  see [Runtime verification](#runtime-verification) below for the exact numbers.

## Round 1 — static review

**#2 — [`extract_faq_pairs`/`faq_metrics` leave stray `**` from bold FAQ labels](https://github.com/Loop-Suite/GEO-Loop/issues/2)**

`src/checks.rs`: both functions stripped `**` only from the very start/end of the whole
line (`trim_start_matches("**").trim_end_matches("**")`). A common bold label format like
`**Q:** What is X?` only wraps the label in `**`, not the whole line, so the trailing `**`
right after the label survived and ended up in the extracted question/answer text — text
that is fed straight into the `geo probe` LLM prompt and into reports.

Fix (`6035491`): added `strip_bold_markers`, which removes all `**` occurrences regardless
of position, used in both functions.

**#3 — [60-point evidence cap documented in README is not enforced anywhere in `score.rs`](https://github.com/Loop-Suite/GEO-Loop/issues/3)**

The README's scoring-pipeline diagram documents that a criterion with no quoted evidence
(< 30 chars) has its score capped at 60. The judge prompt in `build_judge_prompt` asks the
model to self-enforce this, but nothing enforced it server-side: `judge_schema`'s `evidence`
field had no `minLength`, and `score_doc`'s aggregation only did
`x.score.clamp(0.0, 100.0)` — no check on `evidence.len()` at all. A judge model returning a
high score backed by an empty or trivial evidence string would be accepted as-is.

Fix (`aa4b842`): added `minLength: 30` to the `evidence` field in `judge_schema`, and added
`effective_score()` to independently clamp a criterion's score to 60 whenever its (trimmed)
evidence is under 30 characters, regardless of what the model self-reports. `score_doc` now
routes through it.

## Round 2 — deep-dive on the same area

Round 1 touched `checks.rs`'s FAQ-extraction path. Reviewing the rest of that file's text-
matching functions for the same class of problem turned up three more issues.

**#4 — [`answer_summary` check can extract code-fence content as the document's "first paragraph"](https://github.com/Loop-Suite/GEO-Loop/issues/4)**

`checks::first_paragraph()` scanned the raw, un-stripped document. Every other structural
scanner in `checks.rs` (`heading_hierarchy_issues`, `missing_sections`, the section
word-count check, `metrics()`) calls `strip_code_fences()` first so fenced example content
is never mistaken for real structure — `first_paragraph()` was the one exception. Because
its H1 detector only checks "line starts with exactly one `#` followed by text," it also
matched ordinary `#`-comment lines inside fenced Python/Shell/YAML examples. A document with
no real top-level H1 but a fenced comment line would have its real opening paragraph
silently discarded in favor of whatever text followed that fenced comment.

Fix (`1a39dfa`): `first_paragraph()` now scans `strip_code_fences(doc)`, consistent with the
rest of the file.

**#5 — [`missing_sections()` falsely flags a section as missing when heading case differs from spec title](https://github.com/Loop-Suite/GEO-Loop/issues/5)**

`checks::norm()`, used to fuzzy-match document headings against `spec.sections[].title` in
`missing_sections()` and the section length check, only stripped whitespace — it did not
normalize case, unlike `faq_metrics()`'s heading match a few lines below in the same file
(which explicitly lowercases). A heading differing from the spec title only in case (doc has
`## overview`, spec says `"Overview"`) was reported as a missing required section, and its
word count silently skipped in the length check too. **Root cause is distinct from #4/#6** —
missing case-normalization in a comparison function, not a missing `strip_code_fences()`
call — even though it was found in the same review pass over the same file.

Fix (`7c13429`): `norm()` now also lowercases, matching the case-insensitive convention
already used for FAQ heading matching elsewhere in the file.

**#6 — [`geo probe` can extract fake FAQ pairs from a code-fenced example instead of the real FAQ section](https://github.com/Loop-Suite/GEO-Loop/issues/6)**

`probe::run` calls `checks::extract_faq_pairs(doc)` directly on the raw document.
`extract_faq_pairs()` shares its `Q:`/`A:` recognition logic with `faq_metrics()` by its own
doc comment, but the two are called inconsistently: `faq_metrics()`'s only call site
pre-strips fences via `metrics()`; `extract_faq_pairs()`'s call site in `probe.rs` does not.
So while `geo score`'s FAQ-count check correctly ignores `Q:`/`A:`-shaped lines inside fenced
examples, `geo probe` did not — a fenced block demonstrating "how to write your FAQ" (or any
fenced content that happens to start lines with `Q:`/`A:`) was extracted as a real FAQ entry
and sent to the brand-blind `claude -p` probe call, producing a misleading result in
`runs/probe/report.md`. **Same root cause as #4**: a function skipped the shared
`strip_code_fences()` guard that its sibling functions apply.

Fix (`061f9de`): `extract_faq_pairs()` now calls `strip_code_fences()` internally, so it's
correct regardless of whether the caller pre-strips — the same defensive pattern
`heading_hierarchy_issues()` already used.

## Runtime verification

After all 5 fixes landed, `geo score` and `geo probe` were run for real — actual
`claude -p --model haiku --judge-model haiku` subprocess calls, real API cost — against a
single test document engineered to trip every fixed bug at once:

- A fenced bash example containing a fake `#`-style heading and fake statistic-looking
  numbers (targets #4's fence-misread and the statistics check).
- A fenced markdown example containing a fake `Q:`/`A:` block (targets #6).
- 3 real FAQ pairs written with bold labels (`**Q:** ... **A:** ...`) (targets #2).
- Real JSON-LD structured data and an `llms.txt` block (baseline structural requirements,
  not under test).

| Command | Result | Real cost |
|---|---|---|
| `geo score` | 73.8/100 | $0.0515 |
| `geo probe` | 3/3 real FAQ pairs extracted; the fenced fake pair ignored | $0.0197 |
| **Total** | | **$0.0712** |

Measured internal values confirm each fix did what it was supposed to, not just that the
commands didn't crash:

- **`words = 609`** — word count excludes the fenced example content (would be inflated if
  fence-stripping regressed).
- **`stat_tokens = 13`** — excludes the fake percentage-looking numbers inside the bash
  fence (would be inflated if #4's fix regressed on the statistics path).
- **`faq_qa_count = 3`** — exactly the 3 real bold-labeled pairs, with clean text (no stray
  `**`, confirming #2) and no contribution from the fenced fake Q/A pair (confirming #6).
- `geo probe`'s report contains only the 3 real questions — the fenced example pair does not
  appear anywhere in its output.

No bugs were found in this pass. That is the expected outcome for a verification run against
already-fixed code, not a null result — it confirms the 5 static fixes hold under an actual
adversarial document and a real model call, which the static review alone couldn't prove.

## Fix commit reference

| Issue | Title | Fix commit |
|---|---|---|
| [#2](https://github.com/Loop-Suite/GEO-Loop/issues/2) | Bold FAQ label stray `**` | `6035491` |
| [#3](https://github.com/Loop-Suite/GEO-Loop/issues/3) | 60-point evidence cap not enforced | `aa4b842` |
| [#4](https://github.com/Loop-Suite/GEO-Loop/issues/4) | `answer_summary` reads code-fence content | `1a39dfa` |
| [#5](https://github.com/Loop-Suite/GEO-Loop/issues/5) | Case-sensitive section-title matching | `7c13429` |
| [#6](https://github.com/Loop-Suite/GEO-Loop/issues/6) | `geo probe` extracts fake FAQ from code fence | `061f9de` |
