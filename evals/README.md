# Empirical review findings

This documents a two-phase review actually carried out on this repo: (1) a static code
review that filed and fixed 5 issues, and (2) a real CLI execution pass — actual
`claude -p --model haiku --judge-model haiku` subprocess calls, real API cost, not a
simulation — run against an adversarial test document to check whether the 5 fixes hold up
in practice. No bugs were found in phase 2; this is a record of what was checked and what it
cost, not a discovery report.

A follow-up **production hardening round** (see [below](#production-hardening-round)) then
re-audited those fixes adversarially, tripled the test count, cut a tagged release, and ran a
*second* real runtime verification pass against a different adversarial document — which did
find and fix a real bug, one the first runtime pass never exercised.

## TL;DR

| Phase | Scope | Result | Real cost |
|---|---|---|---|
| Round 1 — static review | full repo | 2 issues filed and fixed (#2, #3) | $0 (static) |
| Round 2 — deep-dive on round-1 area | `checks.rs` / `probe.rs` | 3 more issues filed and fixed (#4, #5, #6) | $0 (static) |
| Runtime verification | `geo score` + `geo probe` on a purpose-built adversarial doc | 0 new bugs — all 5 fixes confirmed by measured output | $0.0712 |
| **Round 1 total** | | **5/5 issues fixed, 0 found at runtime** | **$0.0712** |
| Adversarial re-audit | guard-call-site audit + JSON-LD/ReDoS/path-traversal/panic checks | 0 new bugs, no issue filed | $0 (static) |
| Edge-case tests | empty/huge/malformed-JSON-LD/unicode inputs | 29 → 57 tests (+28), [#12](https://github.com/Loop-Suite/GEO-Loop/pull/12) | $0 |
| Versioning | `CHANGELOG.md` + tag | [#13](https://github.com/Loop-Suite/GEO-Loop/pull/13), [`v0.1.0`](https://github.com/Loop-Suite/GEO-Loop/releases/tag/v0.1.0) | $0 |
| Runtime verification, round 2 | `geo score` + `geo probe` on a *different* adversarial doc | 1 real bug found & fixed: system-prompt leak ([#14](https://github.com/Loop-Suite/GEO-Loop/issues/14) / [#15](https://github.com/Loop-Suite/GEO-Loop/pull/15)) | ≈$0.14 |
| **Production hardening round total** | | **1/1 issue found & fixed, tests 29→57** | **≈$0.14** |
| **Grand total** | | **6 issues fixed across both rounds, 57 tests. `v0.1.0` was tagged before #14/#15 landed — patched forward as `v0.1.1`** | **≈$0.21** |

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

## Production hardening round

A second pass, done after the round above shipped as `v0.1.0`: an adversarial re-audit of
the round-1 fixes, a much larger edge-case regression suite, semantic versioning
(`CHANGELOG.md` + tag), and — the part that mattered — a *second* real runtime verification
pass against a different adversarial document than the one used above. That second pass
found a real bug the first runtime pass never exercised.

### 1. Adversarial re-audit of the round-1 fixes (0 new bugs)

Systematically grepped every call site of every `strip_code_fences()`-dependent function in
`checks.rs` — `parse_headings`, `split_sections`, `stat_token_count`, `source_link_count`,
`faq_metrics`, `first_paragraph`, `heading_hierarchy_issues`, `metrics`, `missing_sections`,
`extract_faq_pairs`, `extract_jsonld_blocks` — the same guard-gap class that produced #4 and
#6 above. All current call sites are consistent (each function strips fences internally or
its only caller pre-strips); no further instance found.

Also checked, each independently:

- **JSON-LD resource exhaustion** — built a small scratch Rust project to directly test
  `serde_json` 1.0.151's behavior on abnormally deep nesting and confirmed it already
  enforces a ~128-level recursion limit on both array and object nesting and returns `Err`
  rather than overflowing the stack — defended at the library level, ahead of and
  independent of this project's own `MAX_JSON_DEPTH` guard.
- **ReDoS** — the `regex` crate used throughout has no backtracking engine, so untrusted
  document content can't trigger catastrophic backtracking.
- **Path traversal** — every file path in `main.rs`/`loop_run.rs`/`probe.rs`/`report.rs`/
  `spec.rs` is either a CLI argument supplied by the operator or a label the program
  generates itself; none is derived from untrusted document/spec content.
- **`unwrap`/slicing panics** — audited every call site for panics on adversarial input.

Result: **0 new bugs.** No issue was filed — this is a record of what was checked and the
honest negative result, not a discovery report (same convention as the "no bugs found"
runtime pass in Round 1 above).

### 2. Edge-case regression suite — 29 → 57 tests (PR #12)

28 tests added across `checks.rs`, `llm.rs`, and `probe.rs`, covering:

| Category | Cases |
|---|---|
| Empty / whitespace input | empty doc, whitespace-only doc — metrics, structural scans, `format_issues`, `llms_txt_issues` all checked for panic-free zero/empty output |
| Huge documents | 50,000-line document, a single 500,000-word line, a 20,000-heading list — checked for correct scaling and no panic |
| Malformed JSON-LD | input exceeding serde's own recursion limit, truncated JSON, a scalar JSON-LD root, a non-array `mainEntity`, a wide flat array of 5,000 nodes |
| Extreme Unicode | Arabic RTL text, ZWJ emoji sequences, astral-plane characters, combining diacritics, CRLF line endings — word count / first-paragraph extraction checked for panic-free handling |

PR: [#12](https://github.com/Loop-Suite/GEO-Loop/pull/12) (merged).

### 3. Versioning (PR #13, tag `v0.1.0`)

Added `CHANGELOG.md` (Keep a Changelog format, Added/Fixed/Security sections) and cut the
first tagged release.

PR: [#13](https://github.com/Loop-Suite/GEO-Loop/pull/13) (merged). Release:
[v0.1.0](https://github.com/Loop-Suite/GEO-Loop/releases/tag/v0.1.0).

### 4. Runtime verification, round 2 — a real bug found

Round 1's runtime pass (above) used one adversarial document and found nothing new — a clean
verification result. This round deliberately used a **different** adversarial test document
— a Python code fence with an unlabeled fence type, and an FAQ block written as plain
`Q:`/`A:` text rather than bold-labeled — and ran `geo score` plus `geo probe`
(`--model haiku`) against it for real. This time it found something.

**The bug:** one `geo probe` answer, instead of answering as a brand-blind general user,
opened with "I'll explore the repository to understand..." and emitted literal agentic
tool-call text (`<function_calls><invoke name="bash">...`) into the report.

**Root cause:** `Llm::call_once` (`src/llm.rs`) built every subprocess call with
`--append-system-prompt`, which *appends* this project's `SYSTEM`/`JUDGE_SYSTEM`/
`PROBE_SYSTEM` on top of Claude Code's own default system prompt (identity, cwd, env info,
git status, agentic-coding-tool framing) instead of replacing it. `probe.rs`'s own module doc
promises a "general user with no context," but the default identity underneath was still
there, so the model still believed it was Claude Code operating inside this repo's working
directory.

Confirmed with an isolated `claude -p` repro using the exact flags `call_once` passes:
`--append-system-prompt` left `cache_creation_input_tokens: 4116` (the injected default
prompt) and produced a "let me explore this repository" answer; the same call with
`--system-prompt` (full replace) returned `cache_creation_input_tokens: 0` and a clean,
generic answer.

**Fix:** switch to `--system-prompt` (full replace) — except when `--load-context` is set,
since that flag intentionally omits `--safe-mode` to load `CLAUDE.md`/skills/plugins from the
execution directory, and that injection rides on the default system-prompt pipeline that
`--system-prompt` bypasses entirely. Verified the two flags differ in exactly this way with a
`CLAUDE.md` marker test, so `--load-context` keeps `--append-system-prompt` (conditional fix,
no regression on that path).

Filed as [#14](https://github.com/Loop-Suite/GEO-Loop/issues/14), fixed in
[#15](https://github.com/Loop-Suite/GEO-Loop/pull/15) (merged). Verified end-to-end by
re-running `geo probe` on the same 3 questions after the fix: cumulative cost for the run
dropped from **$0.0360 → $0.0088** (the large default-system-prompt cache injection is gone
from every call).

**Real cost of this pass:**

| Call | Real cost |
|---|---|
| `geo score` | $0.0865 |
| `geo probe` (bug discovery run) | $0.0360 |
| `geo probe` (post-fix verification run) | $0.0088 |
| Isolated `claude -p` repro calls (several, each < $0.01) | ~$0.01–0.02 |
| **Total** | **≈$0.14** |

### `v0.1.0` does not include the #14/#15 fix

`v0.1.0` was tagged at `d8a675f` (PR #13 — the CHANGELOG-only commit), **before #14 was even
filed**. Confirmed directly:

```
$ git log v0.1.0..main --oneline
2c15672 Fix Claude Code's default system prompt leaking into every LLM call (#15)
```

Exactly one commit sits after the tag, and it's the #14/#15 fix. Anyone running `v0.1.0`'s
`geo probe` still has the system-prompt leak. Patched forward as **v0.1.1** — see
[v0.1.1](https://github.com/Loop-Suite/GEO-Loop/releases/tag/v0.1.1).

### Updated totals

| | Round 1 (static + runtime) | Production hardening round | Combined |
|---|---|---|---|
| Issues found & fixed | 5 (#2–#6) | 1 (#14) | 6 |
| Tests | — | 29 → 57 (+28) | 57 |
| Real LLM cost | $0.0712 | ≈$0.14 | ≈$0.21 |
