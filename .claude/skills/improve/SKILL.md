---
name: improve
description: |
  Probabilistic / sampled improvement sweep of the current codebase, project, or app. Spawns N parallel spotter subagents (default 16) that each sample one random aspect (a function, struct, pattern, doc section, project META, etc.) and propose ONE concrete improvement with a rationale. Each spotter report is then adversarially challenged by K more subagents (default 2) that produce their own adjusted proposals (confirm, refine, downgrade, or replace). Finally synthesizes one deduplicated, prioritized improvement list and EXECUTES every accepted improvement via fixer subagents — running them in parallel when their file sets are provably disjoint, serially otherwise. Improvements may target any file (code, docs, configs, schemas, CI, etc.); after execution a drift-reconciliation sweep fixes anything on either side that no longer matches (code ↔ docs, code ↔ configs, etc.).

  Optional args: `/improve` uses defaults; `/improve N` overrides spotter count (e.g. `/improve 8`); `/improve N K` overrides both spotter count and adversaries-per-spotter (e.g. `/improve 32 4`). Add the literal token `dry` anywhere in the args (e.g. `/improve dry`, `/improve 8 dry`, `/improve 32 4 dry`) to stop after synthesis — produce the prioritized plan but do NOT execute any fixer subagents. Useful for previewing before committing to edits.

  TRIGGER on /improve, AND on any natural-language request that asks for open-ended improvement of the project / codebase / repo without specifying a target. Examples that MUST trigger this skill:
    - "improve my codebase / project / repo / code"
    - "find things to fix in this project"
    - "clean up / polish / tighten this codebase"
    - "what should I fix?"
    - "sweep the repo for improvements"
    - any "improve", "polish", "tighten", "clean up", "fix things", "what needs work" phrasing aimed at the codebase as a whole

  DO NOT trigger when: the user names a specific file/function/PR to fix (edit directly), asks about a single concrete bug, invokes /review or /security-review, or asks only for a *rating* (use /rate).

  When inferring, just call the skill — do not ask the user to confirm. The skill itself is the answer to "improve it."
---

# /improve — Probabilistic Project Improvement Sweep

Random-sample improvement pass. The point is **not** completeness. You are deliberately sampling a scattered handful of aspects to surface a mixed bag of improvements, letting adversaries challenge each one, then executing the survivors. Variance and surprise are features, not bugs — the skill makes up for narrow sampling by running repeatedly.

This skill is the action-oriented sibling of `/rate`. Where `/rate` judges, `/improve` fixes.

## Arguments

The skill accepts up to two optional positional numeric arguments (N, K) plus one optional flag token (`dry`):

- `/improve` → defaults: **N = 16** spotters, **K = 2** adversaries per spotter, dry mode **off** (fixes are executed)
- `/improve N` → **N** spotters, still **K = 2** adversaries per spotter (e.g. `/improve 8` → 8 spotters × 2 adv = 24 proposals)
- `/improve N K` → **N** spotters, **K** adversaries each (e.g. `/improve 32 4` → 32 spotters × 4 adv = 160 proposals)
- `/improve dry` → defaults + **dry mode on** (produce the prioritized improvement plan, stop before execution)
- `/improve N dry` or `/improve N K dry` → same N/K override as above, plus dry mode on

Parsing rules:

1. Split the argument string on whitespace.
2. If any token equals `dry` (case-insensitive), set **dry mode = on** and remove that token.
3. Parse the remaining tokens positionally as N then K. Tokens that are missing, non-numeric, or not an integer (e.g. `8.5`, `foo`) use the default; tokens beyond the second are ignored. Then clamp N ≥ 1 and K ≥ 0 (so `0` and negatives parse as valid integers and get clamped afterwards — and K = 0 is legal, meaning no adversarial pass, just raw spotter proposals).

The `dry` token may appear before, between, or after the numeric args — `/improve dry 8`, `/improve 8 dry`, and `/improve 8 2 dry` are all equivalent to N=8, K=2, dry=on.

## Flow at a glance

1. Spawn **N spotter subagents** in parallel (background). Each picks one random target and reports a single concrete improvement proposal.
2. As each spotter report arrives, immediately spawn **K adversarial subagents** in parallel (background) that independently produce adjusted versions of that proposal. (If K = 0, skip this step.)
3. When all N originals have all K adversarial adjustments back (= **N·(K+1) total proposals, in N tuples of size K+1**), synthesize one deduplicated, prioritized improvement plan.
4. **Execute** the plan: dispatch fixer subagents for each accepted improvement. Batch improvements whose file sets are provably disjoint into parallel waves; run anything that might touch overlapping files serially. (Skipped when dry mode is on.)
5. After execution, run a **drift-reconciliation sweep** to bring everything that references the edited files back into sync — in either direction (code ↔ docs, code ↔ configs, schemas, examples, CI, etc.). Skipped when dry mode is on, or when Step 4 edited no files. Then report results to the user.

Do not wait for all spotters before launching adversaries. Eager spawning is a hard requirement whenever K ≥ 1 (when K = 0 there are no adversaries, so this rule is vacuous).

---

## Step 1 — Launch N spotters in parallel

In a single message, call the Agent tool **N times** with `subagent_type: general-purpose` and `run_in_background: true`. Give each spotter a different **category hint** so the sample spreads across the project. Suggested pool — if N ≤ pool size, sample N distinct hints; if N > pool size, use each hint once and fill the remainder with the wildcard:

- a specific function or method
- a specific struct / class / type / enum
- a specific module or single source file
- a recurring pattern (error handling, logging, validation, retries, ID generation, etc.)
- a style/formatting choice (naming, comments, line length, import ordering)
- a structural decision (folder layout, module boundaries, dependency graph)
- an architectural approach (concurrency model, state management, data flow)
- a section of a doc file (README, CLAUDE.md, design doc, ADR)
- project META: `.gitignore`, CI config, Dockerfile, build scripts, release process
- dependency choices (what's pulled in, version pinning, alternatives ignored)
- test coverage or quality for some specific area
- API/CLI surface ergonomics
- handling of a specific edge case or failure mode
- security posture of one specific surface
- performance characteristics of one specific path
- a wildcard the spotter finds interesting

**Prompt template for each spotter** (customize the category hint per agent):

> You are a spotter in a probabilistic project improvement sweep. The repo is at the working directory.
>
> **Your category:** `<CATEGORY HINT>`. Find ONE concrete target in this category by exploring the repo. Pick something specific — not "the codebase" but e.g. "the `reconcile_index` function in src/store/index.rs", or "the error type hierarchy in src/error.rs", or "the section of README.md describing the storage layout".
>
> Read the actual code/file. Form an opinion about what could be improved. Propose ONE concrete, actionable improvement. Then return EXACTLY this format and nothing else:
>
> ```
> TARGET: <one-line description of what you examined, with file path>
> SEVERITY: <one of: critical | high | medium | low | nit>
> CATEGORY: <one of: correctness | security | performance | clarity | tests | docs | style | deps | build | other>
> FILES: <comma-separated list of every file path this improvement would create or edit — including brand-new files. Always list paths; never "none".>
> PROPOSAL: <2-5 sentences. Concrete change. Name the file and line range. State the exact edit (rename, delete, split, add a test, replace X with Y, etc.) — not "consider improving error handling" but "replace `c.is_alphanumeric()` at validation.rs:24 with `c.is_ascii_alphanumeric()` so Cyrillic lookalikes are rejected at parse time".>
> RATIONALE: <1-3 sentences. Why this matters. What breaks or degrades today.>
> ```
>
> Severity calibration (be honest, not diplomatic):
> - **critical** — bug that corrupts data, leaks secrets, or crashes on normal input
> - **high** — real correctness/security/perf problem a maintainer would want to know about
> - **medium** — clear defect or obvious quality win; worth fixing soon
> - **low** — minor nit, readability, small cleanup
> - **nit** — cosmetic / taste-level; fixing is fine, skipping is fine
>
> If after reading you genuinely find nothing worth changing, return `PROPOSAL: none` and nothing else in that field (leave the other fields filled in so we know what you looked at). Do not invent improvements to fill a quota.

## Step 2 — Spawn adversaries eagerly as each spotter returns

Skip this step entirely if K = 0 — go directly to Step 3 and treat each spotter's proposal as its final proposal.

You will receive a notification each time a background spotter finishes. The instant a spotter's report arrives:

- In your **next turn**, send a single message with **K Agent calls** (parallel, all `run_in_background: true`) for that report.
- Continue handling other spotter notifications as they come in. Do NOT batch — interleave eagerly.

Each adversary gets the same prompt independently:

> You are an adversarial reviewer in a probabilistic project improvement sweep. Another spotter produced this proposal:
>
> ```
> <PASTE ORIGINAL REPORT VERBATIM>
> ```
>
> Your job: **poke holes**. Read the same target yourself. Decide whether the proposed change is: (a) correct and worth doing, (b) correct but wrong-priority (severity mis-set), (c) subtly incorrect — would regress something or break a caller, (d) redundant — already done elsewhere, or (e) replaceable by a better fix for the same underlying issue.
>
> Look for:
> - context the original spotter missed (callers, tests, invariants, history, related files)
> - whether the "improvement" would actually regress behavior
> - whether a smaller/cheaper fix addresses the same root cause
> - whether the severity/category labels match the real impact
> - whether the FILES list is accurate (missing or spurious files)
>
> Then produce YOUR OWN adjusted report in EXACTLY the same format:
>
> ```
> TARGET: <same target, possibly refined>
> SEVERITY: <your adjusted severity>
> CATEGORY: <your adjusted category>
> FILES: <your adjusted file list>
> PROPOSAL: <your adjusted concrete change — may confirm, refine, or replace>
> RATIONALE: <1-3 sentences. What the original missed or got wrong. Why your proposal is more accurate.>
> VERDICT: <one of: confirm | refine | replace | reject>
> ```
>
> VERDICT calibration:
> - **confirm** — the original is right; your PROPOSAL is essentially the same, maybe wording-tightened
> - **refine** — same underlying issue, but the fix should be different (different files, different approach, different severity)
> - **replace** — the target has a real issue but the original picked the wrong one; propose the better fix
> - **reject** — no real issue here, or the proposed fix would regress; set PROPOSAL to `none` and explain in RATIONALE
>
> No rubber-stamping — only `confirm` when after genuine adversarial scrutiny you still agree.

Adversaries run independently; when K ≥ 2 they may disagree with each other, which is fine.

## Step 3 — Synthesis: build the prioritized improvement plan

Once every original has all its adversarial adjustments back (N tuples of size K+1, N·(K+1) proposals total), build the plan.

### 3a. Collect

Gather every PROPOSAL from all N·(K+1) reports. Drop any with `PROPOSAL: none` (spotter found nothing, or adversary marked `reject`). What remains is the raw candidate set.

### 3b. Resolve adversary verdicts per tuple

For each of the N original tuples (spotter + its K adversaries), before any cross-tuple merging:

- If **> half** the adversaries voted `reject` (strict majority), drop the spotter's proposal from this tuple's contribution to the candidate set. (With K = 2, that means 2 of 2; with K = 3, ≥ 2; with K = 4, ≥ 3.) It may still survive if a *different* tuple independently surfaced the same fix — that's handled by 3c.
- If an adversary voted `replace`, their PROPOSAL is a separate candidate alongside (or instead of) the original.
- If verdicts are mixed `confirm` / `refine`, keep the refined version and record the disagreement to surface in the plan.
- If K = 0, every spotter proposal passes straight through.

### 3c. Deduplicate across tuples

Group the surviving candidates that target **the same underlying change** — same file(s) + same semantic edit, even if worded differently. Within a group:

- Prefer the most specific / concrete wording.
- Track **multiplicity** — how many distinct agents (across tuples) surfaced this fix. High multiplicity is strong signal.
- Resolve severity/category conflicts by taking the **most severe** severity any non-rejected agent assigned (a single "critical" outranks three "low"s).
- A candidate that was rejected on one tuple but confirmed on another survives — the independent confirmation is what matters.

### 3d. Prioritize

Assign each surviving candidate to one of:

- **P0 — Do now.** Severity critical or high AND either (a) multiplicity ≥ 2 OR (b) when K ≥ 1, confirmed by every adversary on its tuple AND not rejected by any adversary elsewhere. Correctness/security issues live here by default.
- **P1 — Do in this sweep.** Severity medium, or severity high that didn't clear the P0 bar. Clear, concrete, low-regression-risk changes.
- **P2 — Do if cheap.** Severity low or nit. Cosmetic/taste cleanups land here.
- **Skip.** Rejected by adversaries, or the adversary's replacement is also in the plan and supersedes it, or the proposal is vague after consolidation.

When K = 0 there are no adversary verdicts, so priority is driven by severity and multiplicity alone (rule (a) above still works; rule (b) is vacuous).

### 3e. Render the plan to the user

Before any execution, print the plan. Up to three sections, in order (the middle one is omitted when K = 0):

**Per-tuple overview** — a compact table, one row per original spotter target, showing what the tuple converged on:

| # | Target | Original severity | Adv A verdict | Adv B verdict | … | Final status |
|---|--------|-------------------|---------------|---------------|---|--------------|

`Final status` is one of: `→ P0`, `→ P1`, `→ P2`, `→ Skip`, or `→ merged with #M` (when deduped into another row).

**Notable disagreements** — 3–5 bullets on the most interesting cases where adversaries changed the outcome materially (downgraded a proposal, replaced it with a better fix, rejected a confident spotter, or disagreed with each other when K ≥ 2). One line each. Omit when K = 0.

**The plan** — three lists (P0, P1, P2) of improvements to execute. Each bullet has: severity, category, file(s), a one-line description, and a `(flagged by M agents)` parenthetical when M ≥ 2. Skip any empty priority group. Example:

> **P0 — Do now**
> - **[high / correctness]** [src/validation.rs:24](src/validation.rs#L24) — replace `c.is_alphanumeric()` with `c.is_ascii_alphanumeric()` so Cyrillic lookalikes are rejected at parse time _(flagged by 3 agents)_
>
> **P1 — Do in this sweep**
> - **[medium / tests]** [tests/store_test.rs](tests/store_test.rs) — add a regression test for the duplicate-id edge case described in the reconcile path
>
> **P2 — Do if cheap**
> - **[nit / style]** [src/error.rs:12-18](src/error.rs#L12-L18) — collapse the three near-identical `From` impls with a macro

If no candidates survived, print `_No actionable improvements surfaced in this sample._` and stop.

**If dry mode is on, stop here.** Do not execute anything. Remind the user that `/improve` without `dry` would execute this plan, BUT that re-running `/improve` produces a different probabilistic sample — so running `/improve` again after `/improve dry` will not reproduce the exact same plan, it will sample afresh. If they want *this* specific plan applied, they should act on it now (and can run `/improve dry` again later for a new set of candidates).

---

## Step 4 — Execute the plan

Execute **P0 and P1** improvements by default. Execute **P2** too unless P0+P1 alone would already touch more than ~10 files (in which case list the P2 items as deferred and invite the user to re-run). This keeps any single sweep's blast radius bounded.

### 4a. Group for parallel execution

For each improvement to execute, you already have its `FILES` list from the proposal. Build a list of `(improvement_id, files_touched)` tuples. Group into **waves** such that within one wave no two improvements share any file path, and any improvement whose files are unclear/unknown is placed in its own singleton wave (serial).

Algorithm (greedy, good enough):

1. Sort improvements by priority (P0 before P1 before P2), then by number of files touched (fewer first).
2. Walk the list. For each improvement, try to add it to the earliest existing wave whose union of files has no intersection with this improvement's files. If no wave fits, open a new wave.
3. A brand-new file (a path in FILES that does not yet exist) is treated like any other entry — two fixers both creating the *same* new path collide, two fixers creating different new paths do not. Fixers never touch files outside their declared FILES list, so any module registration (imports, `mod` declarations, index entries) that a new file needs is either already in FILES (fine), or will be handled by the Step 5 drift sweep after the fixer returns `STATUS: partial`.

**Be conservative.** If you are not *certain* two improvements touch disjoint files, put them in separate waves. A wrongly-parallelized pair of fixers can clobber each other's edits; the cost of a serial step is much lower than the cost of a merge conflict inside a subagent's diff.

### 4b. Dispatch fixer subagents per wave

For each wave, in a single message send one Agent call per improvement in the wave, all `subagent_type: general-purpose` and all `run_in_background: true`. Wait for the wave to finish before starting the next. (Parallelism within a wave; serial between waves.)

**Prompt template for each fixer:**

> You are a fixer subagent in an improvement sweep. Implement EXACTLY the following improvement and nothing else. Do not expand scope, do not refactor nearby code, do not "tidy up" adjacent lines.
>
> **Improvement to apply:**
>
> ```
> <PRIORITY> / <SEVERITY> / <CATEGORY>
> FILES: <comma-separated file list>
> CHANGE: <the concrete proposal text>
> RATIONALE: <why>
> ```
>
> Rules:
> 1. Read the file(s) first to confirm the described state still matches reality. If it does not (someone else has since changed it, the line numbers have shifted, the function was renamed), adapt the change to the new reality only if the intent still clearly applies. If the intent no longer applies at all, abort and report `STATUS: obsolete` with a one-line explanation.
> 2. Make the minimum edit that implements the change. Preserve surrounding style (indentation, quoting, naming conventions).
> 3. **Stay strictly within the declared FILES list.** You may only create or modify files that appear in that list. You may freely edit within those files — including same-file doc comments, inline examples, and in-file schemas — but do NOT edit any path outside that list, even if you believe it has drifted. Cross-file drift is reconciled in a dedicated sweep after you return. If you discover an edit outside your list is urgently needed, DO NOT make it; record it in `FOLLOWUPS` and set `STATUS: partial`.
> 4. If the change is a code edit, also run any obviously-relevant local checks the repo uses (e.g. if there's a `cargo check` / `npm run typecheck` convention you can see from scripts, run it on the edited crate/package — do not run the full test suite). Do NOT attempt to fix unrelated pre-existing failures.
> 5. Do not commit. Leave edits in the working tree.
>
> Return EXACTLY this format and nothing else:
>
> ```
> STATUS: <done | partial | obsolete | blocked>
> FILES_EDITED: <comma-separated paths, or "none"; MUST be a subset of the declared FILES list>
> SUMMARY: <1-3 sentences on what you actually changed, or why you couldn't>
> FOLLOWUPS: <comma-separated notes on drift or adjacent issues you noticed but did NOT act on, or "none">
> ```

### 4c. Between waves

Before starting the next wave, briefly (one or two lines) tell the user which improvements landed, which were obsolete or blocked, and which wave is starting next. This keeps the user oriented during long sweeps without flooding the chat.

---

## Step 5 — Drift-reconciliation sweep

After all waves complete (and if dry mode is off), run one final pass to catch drift the individual fixers were forbidden from touching. This step is mandatory whenever Step 4 edited at least one file. It is the ONLY place cross-file drift is allowed to be fixed.

### 5a. Decide whether to run

1. Collect the union of all `FILES_EDITED` across every fixer, plus any follow-up drift hints from the `FOLLOWUPS` fields.
2. If the union is empty (every fixer was `obsolete`/`blocked` with no edits), skip this step and go to Step 6.
3. Otherwise, run the drift subagent below. Drift is bidirectional, so do NOT skip based on file types — an edited config, schema, or doc can just as easily obsolete code as the reverse.

### 5b. Dispatch the drift subagent

Dispatch one drift-reconciliation subagent with `subagent_type: general-purpose` (foreground is fine — it's a single agent and subsequent steps depend on its output):

> You are a drift-reconciliation subagent. The following files were just edited in this repo and represent the **new source of truth**:
>
> ```
> <LIST OF FILES_EDITED, with the matching fixer SUMMARY for each>
> ```
>
> Follow-up drift hints reported by fixers that deliberately did not act on them:
>
> ```
> <FOLLOWUPS, or "none">
> ```
>
> Your job: find every *other* file in the repo — in either direction — whose content is now inconsistent with these edits, and update it so consistency is restored. Drift is bidirectional:
>
> - Code was edited → docs, examples, fixtures, configs, schemas, CLI reference, changelog entries, and tests that name the old symbols/behavior may need updating.
> - Docs / configs / schemas / CI / fixtures were edited → code that reads those keys, implements those flags, parses those schemas, or fulfills those documented contracts may need updating so it matches the new authoritative spec.
> - A renamed identifier, moved file, or removed flag may leave stale references in *any* kind of file — grep for the old names, not just one category.
>
> Rules:
> 1. Treat the files listed above as authoritative. Do NOT modify them further. Only edit files whose content disagrees with them.
> 2. Only fix drift that is a direct consequence of the listed edits. Do NOT fix unrelated pre-existing drift, do NOT refactor, do NOT restructure, do NOT invent new documentation.
> 3. When drift points in an ambiguous direction (both sides could plausibly be "right"), default to trusting the edited side — it was chosen deliberately by the improvement sweep. If you genuinely cannot tell, leave both untouched and flag it in `UNRESOLVED`.
> 4. If a fixer's edit appears incorrect in light of what you find elsewhere, DO NOT revert it — flag it in `UNRESOLVED` and let the user decide.
> 5. Do not commit.
>
> Return EXACTLY this format and nothing else:
>
> ```
> FILES_UPDATED: <comma-separated paths you edited, or "none">
> CHANGES: <one short bullet per file describing what you reconciled>
> UNRESOLVED: <comma-separated notes on drift you could not confidently fix, or "none">
> ```

Fold the drift-reconciliation result into the final report.

---

## Step 6 — Final report to the user

After execution and the drift-reconciliation sweep finish, print a tight summary in chat (no file):

### Executed

A table: one row per improvement that was attempted, with columns `#`, `Priority`, `Target`, `Status`, `Files edited`.

### Obsolete / blocked / skipped

Bulleted list of anything the plan included but execution didn't complete, with the reason the fixer returned. One line each.

### Drift reconciled

Bulleted list of every file touched by the Step 5 sweep (code, docs, configs, schemas, anything), each with a short note on what was reconciled. If the sweep ran but found nothing to reconcile, state that explicitly. If the sweep did not run (no files edited in Step 4), omit this section.

### Unresolved

Anything in the drift subagent's `UNRESOLVED` field — cases where drift was detected but could not be confidently auto-reconciled, and the user should make the call. Omit when empty.

### Follow-ups worth considering

Aggregate every non-empty `FOLLOWUPS` from the fixer reports that the drift sweep did NOT already resolve. Deduplicate. Render as a short bulleted list. These are deliberately *not* executed in this sweep — they are hints for the next `/improve` run or for a focused fix.

### Closing note

One sentence: `_This was a random sample, not a complete pass — re-run /improve for a different sample._`

---

## Hard rules

- **N spotters, parallel, background.** One message, N Agent calls. Default N = 16; override from first positional argument.
- **K adversaries per spotter, eager.** Spawn them the moment the spotter returns, not in a batch at the end. Default K = 2; override from second positional argument. K = 0 skips the adversarial pass entirely.
- **Dry mode is a flag** toggled by the literal token `dry` anywhere in the args. When on, stop after Step 3 (the plan is printed; no fixers run, no drift sweep runs). When off, execute the plan.
- **Adversaries read the target themselves** — they don't just argue from the original's text.
- **Spotters and adversaries don't edit files.** They only propose. Editing happens in Step 4 via dedicated fixer subagents.
- **Fixers stay in scope.** Each fixer implements exactly one improvement, no drive-by cleanup. Scope creep inside a fixer is the single most common way these sweeps go wrong.
- **Parallel only when provably disjoint.** If two improvements' file sets overlap, or if a file set is unknown/uncertain, the improvements go in different waves. Conservative wins; a wrong parallelization corrupts someone's edits.
- **Drift reconciliation is mandatory.** Whenever Step 4 edited at least one file, run the Step 5 drift sweep. Drift is bidirectional — do not skip just because "only docs changed" or "only code changed." Fixers are forbidden from touching undeclared files, so Step 5 is the only place cross-file reconciliation happens.
- **Don't pre-fetch** code for spotters or adversaries. They do their own reading; that's part of the sample.
- **Don't commit.** All edits stay in the working tree for the user to review, test, and commit.
