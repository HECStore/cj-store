---
name: improve
description: |
  Probabilistic / sampled improvement sweep of the current codebase, project, or app. Spawns N parallel spotter subagents (default 16) that each sample one random aspect (a function, struct, pattern, doc section, project META, etc.) and propose 1+ concrete improvements with rationale — one big change, several smaller ones, or a mix, depending on what they actually find in the sampled area. Each spotter report is then adversarially challenged by K more subagents (default 2) that read the same target and produce their own adjusted list — confirming, refining, replacing, or rejecting each proposal, and adding any the spotter missed. Finally synthesizes one deduplicated, prioritized improvement list and EXECUTES every accepted improvement via fixer subagents — running them in parallel when their file sets are provably disjoint, serially otherwise. Improvements may target any file (code, docs, configs, schemas, CI, etc.); after execution a drift-reconciliation sweep fixes anything on either side that no longer matches (code ↔ docs, code ↔ configs, etc.).

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
- `/improve N` → **N** spotters, still **K = 2** adversaries per spotter (e.g. `/improve 8` → 8 spotter lists, each reviewed by 2 adversaries)
- `/improve N K` → **N** spotters, **K** adversaries each (e.g. `/improve 32 4` → 32 spotter lists, each reviewed by 4 adversaries)
- `/improve dry` → defaults + **dry mode on** (produce the prioritized improvement plan, stop before execution)
- `/improve N dry` or `/improve N K dry` → same N/K override as above, plus dry mode on

Parsing rules:

1. Split the argument string on whitespace.
2. If any token equals `dry` (case-insensitive), set **dry mode = on** and remove that token.
3. Parse the remaining tokens positionally as N then K. Tokens that are missing, non-numeric, or not an integer (e.g. `8.5`, `foo`) use the default; tokens beyond the second are ignored. Then clamp N ≥ 1 and K ≥ 0 (so `0` and negatives parse as valid integers and get clamped afterwards — and K = 0 is legal, meaning no adversarial pass, just raw spotter proposals).

The `dry` token may appear before, between, or after the numeric args — `/improve dry 8`, `/improve 8 dry`, and `/improve 8 2 dry` are all equivalent to N=8, K=2, dry=on.

## Flow at a glance

1. Spawn **N spotter subagents** in parallel (background). Each picks one random target area and reports 1+ concrete improvement proposals (one big change, several smaller ones, or a mix — quality over quantity).
2. As each spotter report arrives, immediately spawn **K adversarial subagents** in parallel (background) that independently review the spotter's *whole list* and produce their own adjusted lists. (If K = 0, skip this step.)
3. When all N originals have all K adversarial adjustments back (each tuple = one spotter list + K adversary lists; total proposal count is variable), synthesize one deduplicated, prioritized improvement plan.
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

**Prompt template for each spotter** (customize the category hint and SEED per agent — every subagent across the entire run, spotters/adversaries/fixers/drift alike, must receive a *unique* SEED of 8 freshly-generated random English words; do not reuse seeds across subagents and do not explain the field to them):

> SEED: `<EIGHT RANDOM WORDS, SPACE-SEPARATED>`
>
> You are a spotter in a probabilistic project improvement sweep. The repo is at the working directory.
>
> **Your category:** `<CATEGORY HINT>`. Find one concrete target area in this category by exploring the repo. Pick something specific — not "the codebase" but e.g. "the `reconcile_index` function in src/store/index.rs", or "the error type hierarchy in src/error.rs", or "the section of README.md describing the storage layout".
>
> Read the actual code/file. Form an opinion about what could be improved in this target area. Propose **between 1 and ~5 concrete, actionable improvements** for this target — pick whatever genuinely fits what you found:
> - one large/structural change if the area really has a single big issue,
> - several smaller independent changes if you find a handful of unrelated nits/wins,
> - or a mix of one big and a few small.
>
> **Quality over quantity.** Do not pad to hit a number. If only one thing is worth changing, propose only one. If five distinct things are worth changing, propose five. Each proposal will be applied by a separate fixer subagent later. **List proposals in the order you'd want them applied** — earlier numbered proposals are scheduled in earlier execution waves, so later siblings run against the post-edit state of earlier ones. Make each proposal robust to the others succeeding, failing, or being deemed obsolete; don't write proposal 3 in a way that strictly requires proposals 1 and 2 to both have landed.
>
> Return EXACTLY this format and nothing else:
>
> ```
> TARGET_AREA: <one-line description of what you examined, with file path(s)>
>
> PROPOSAL 1
> SEVERITY: <one of: critical | high | medium | low | nit>
> CATEGORY: <one of: correctness | security | performance | clarity | tests | docs | style | deps | build | other>
> FILES: <comma-separated list of every file path this proposal would create or edit, **including required companions** — imports, `mod` declarations, module/index registrations, schema files, fixtures, generated artifacts, and any other path the change can't land cleanly without. Always list paths; never "none". An incomplete FILES list forces the fixer to either report `partial` (handing reconciliation to the drift sweep — fine, but slower) or `blocked` (losing the improvement entirely if a verification check fails on the half-applied state).>
> CHANGE: <2-5 sentences. Concrete change. Name the file and line range. State the exact edit (rename, delete, split, add a test, replace X with Y, etc.) — not "consider improving error handling" but "replace `c.is_alphanumeric()` at validation.rs:24 with `c.is_ascii_alphanumeric()` so Cyrillic lookalikes are rejected at parse time".>
> RATIONALE: <1-3 sentences. Why this matters. What breaks or degrades today.>
>
> PROPOSAL 2
> SEVERITY: ...
> CATEGORY: ...
> FILES: ...
> CHANGE: ...
> RATIONALE: ...
>
> (repeat PROPOSAL N blocks as needed)
> ```
>
> Number proposals starting at 1 with contiguous integers. Separate consecutive PROPOSAL blocks with one blank line. Within a single proposal, every field (SEVERITY, CATEGORY, FILES, CHANGE, RATIONALE) is mandatory.
>
> Severity calibration (be honest, not diplomatic):
> - **critical** — bug that corrupts data, leaks secrets, or crashes on normal input
> - **high** — real correctness/security/perf problem a maintainer would want to know about
> - **medium** — clear defect or obvious quality win; worth fixing soon
> - **low** — minor nit, readability, small cleanup
> - **nit** — cosmetic / taste-level; fixing is fine, skipping is fine
>
> If after reading you genuinely find nothing worth changing, return `TARGET_AREA: <what you looked at>` followed by `PROPOSALS: none` on its own line and nothing else. Do not invent improvements to fill a quota.

## Step 2 — Spawn adversaries eagerly as each spotter returns

Skip this step entirely if K = 0 — go directly to Step 3 and treat each of the spotter's proposals as final.

You will receive a notification each time a background spotter finishes. The instant a spotter's report arrives:

- In your **next turn**, send a single message with **K Agent calls** (parallel, all `run_in_background: true`) for that report.
- Continue handling other spotter notifications as they come in. Do NOT batch — interleave eagerly.

Each adversary reviews the spotter's *entire list* (1 to ~5 proposals) and gets the same prompt independently (each adversary also receives its own freshly-generated unique SEED of 8 random English words — distinct from every other subagent in the run, no explanation given to the subagent):

> SEED: `<EIGHT RANDOM WORDS, SPACE-SEPARATED>`
>
> You are an adversarial reviewer in a probabilistic project improvement sweep. Another spotter produced the following report on a target area, containing one or more proposals:
>
> ```
> <PASTE ORIGINAL REPORT VERBATIM>
> ```
>
> Your job: **poke holes** at every proposal in the list. Read the same target yourself. For EACH numbered proposal in the spotter's list, decide whether the proposed change is: (a) correct and worth doing, (b) correct but wrong-priority (severity mis-set), (c) subtly incorrect — would regress something or break a caller, (d) redundant — already done elsewhere, or (e) replaceable by a better fix for the same underlying issue. You may also surface proposals the spotter **missed** in this same target area, as additions.
>
> Look for:
> - context the original spotter missed (callers, tests, invariants, history, related files)
> - whether any "improvement" would actually regress behavior
> - whether a smaller/cheaper fix addresses the same root cause
> - whether the severity/category labels match the real impact
> - whether each FILES list is accurate (missing or spurious files)
> - whether two of the spotter's proposals secretly overlap and should collapse
> - whether the spotter overlooked an important issue in the same target area
>
> Then produce YOUR OWN adjusted list. Emit one ADJUSTED block per spotter proposal (in the same numeric order, referencing it by `ORIGINAL_REF: <N>`), plus one ADJUSTED block per addition you want to surface (with `ORIGINAL_REF: new`). Format:
>
> ```
> TARGET_AREA: <same target, possibly refined>
>
> ADJUSTED 1
> ORIGINAL_REF: 1
> VERDICT: <one of: confirm | refine | replace | reject>
> SEVERITY: <your adjusted severity>
> CATEGORY: <your adjusted category>
> FILES: <your adjusted file list>
> CHANGE: <your adjusted concrete change — may confirm, refine, or replace; for reject, write "none">
> RATIONALE: <1-3 sentences. What the original missed or got wrong. Why your proposal is more accurate. For reject, explain why no real issue exists.>
>
> ADJUSTED 2
> ORIGINAL_REF: 2
> VERDICT: ...
> ...
>
> ADJUSTED 3
> ORIGINAL_REF: new
> VERDICT: addition
> SEVERITY: ...
> CATEGORY: ...
> FILES: ...
> CHANGE: ...
> RATIONALE: <why this is worth surfacing and the spotter missed it>
> ```
>
> VERDICT calibration:
> - **confirm** — the original is right; your CHANGE is essentially the same, maybe wording-tightened
> - **refine** — same underlying issue, but the fix should be different (different files, different approach, different severity)
> - **replace** — the target has a real issue but the original picked the wrong one; propose the better fix
> - **reject** — no real issue here, or the proposed fix would regress; set CHANGE to `none` and explain in RATIONALE
> - **addition** — used only with `ORIGINAL_REF: new`; you found a separate issue the spotter didn't surface
>
> Rules:
> - You MUST emit one ADJUSTED block for every spotter proposal (`ORIGINAL_REF: 1` through `ORIGINAL_REF: <last>`), in the same numeric order. Skipping a proposal is not allowed — if you have nothing to add, vote `confirm`.
> - Additions are optional. Use them sparingly — only when the omission is genuinely worth surfacing. Do not pad.
> - If the spotter's report was `PROPOSALS: none`, you may still add `ORIGINAL_REF: new` blocks if you spot something they missed; otherwise return `TARGET_AREA: <same>` followed by `ADJUSTMENTS: none`.
> - No rubber-stamping — only `confirm` when after genuine adversarial scrutiny you still agree.

Adversaries run independently; when K ≥ 2 they may disagree with each other, which is fine.

## Step 3 — Synthesis: build the prioritized improvement plan

Once every original has all its adversarial adjustments back (N tuples; each tuple = one spotter list + K adversary lists; total proposal count is variable), build the plan.

### 3a. Collect

Gather every CHANGE from every PROPOSAL block (in spotter reports) and every ADJUSTED block (in adversary reports). Drop anything that is `PROPOSALS: none`, `ADJUSTMENTS: none`, or `CHANGE: none` (spotter found nothing, or adversary marked `reject`). What remains is the raw candidate set, where each candidate is a single proposal (not a tuple).

### 3b. Resolve adversary verdicts per proposal

For each spotter proposal in each tuple, look at the K adversaries' ADJUSTED blocks that reference it by `ORIGINAL_REF: <its number>`. Verdict counting reads the raw adversary outputs (every ADJUSTED block, including rejects whose CHANGE was filtered to "none" in 3a), not the post-3a candidate set. Then, before any cross-tuple merging, the verdicts decide what survives from that proposal:

- **Strict-majority reject** (> half of the K reviewing adversaries voted `reject`): drop everything tied to that proposal in this tuple — the spotter's CHANGE plus every adversary `confirm` / `refine` / `replace` CHANGE for the same `ORIGINAL_REF`. (K = 2 → 2 of 2; K = 3 → ≥ 2; K = 4 → ≥ 3.) Adversary additions on the same tuple (`ORIGINAL_REF: new`) are independent and survive — they aren't tied to the rejected proposal. The dropped change may still resurface from a *different* tuple's spotter or adversary that independently surfaced the same fix — 3c handles that.
- **Otherwise the proposal survives**, and the per-adversary verdicts shape what the surviving candidate looks like:
  - `confirm` — the adversary vouches for the spotter's CHANGE. Their block joins the spotter's into one logical candidate (multiplicity counts both distinct agents). The spotter's wording is kept.
  - `refine` — the adversary agrees there's a real issue but proposes a refined fix. Their block joins the spotter's into one logical candidate (multiplicity counts both). The adversary's wording supersedes the spotter's. When verdicts on the same proposal are mixed (`confirm` + `refine`), keep the refined wording and surface the disagreement in the plan.
  - `replace` — the adversary thinks the spotter picked the wrong fix for a real issue. Their CHANGE enters the candidate set as its own separate entry alongside the spotter's, with its own multiplicity-1 starter count. 3c's cross-tuple dedup later decides whether the replace happens to coincide with another candidate (multiplicity rises, more specific wording wins) or stays distinct (both compete in the plan).
- **Adversary additions** (`ORIGINAL_REF: new`, `VERDICT: addition`): each one enters the candidate set as its own proposal with multiplicity 1. There is no per-proposal adversarial check (it surfaces only on this tuple, with its single voucher), unless 3c's cross-tuple dedup pairs it with an independently-surfaced match elsewhere.
- **K = 0**: there are no adversary verdicts. Every spotter proposal passes straight through with multiplicity 1.

### 3c. Deduplicate across proposals

Group the surviving candidates (spotter proposals, adversary refines/replaces, and adversary additions) that target **the same underlying change** — same file(s) + same semantic edit, even if worded differently. This dedup runs across all tuples *and* across all proposal slots within a tuple. Within a group:

- **Automatic intra-tuple grouping** (no judgment call): a spotter `PROPOSAL N` and every adversary `ADJUSTED` block with `ORIGINAL_REF: N` from the *same tuple* whose verdict is `confirm` or `refine` are by definition the same logical candidate — group them automatically. Cross-tuple grouping, and matching `replace` / adv-add CHANGEs against unrelated candidates, still requires judgment (same file(s) + same semantic edit).
- Prefer the most specific / concrete wording (with the refine-supersedes-confirm rule from 3b already applied within each automatic intra-tuple group).
- Track **multiplicity** as the number of *distinct source agents* (e.g., `tuple-3-spotter`, `tuple-3-adv-A`, `tuple-7-adv-B`) that contributed a CHANGE to this group. The same agent contributing two CHANGE entries (e.g., a spotter who emitted two proposals that turned out to overlap) counts once, not twice. High multiplicity is strong signal.
- Resolve severity/category conflicts by taking the **most severe** severity any non-rejected agent assigned (a single "critical" outranks three "low"s).
- A candidate that was rejected on one tuple but confirmed on another survives — the independent confirmation is what matters. The rejecting tuple's reject does NOT reduce multiplicity of the merged group; only contributing agents count.

### 3d. Prioritize

Assign each surviving candidate to one of:

- **P0 — Do now.** Severity critical or high AND multiplicity ≥ 2 (corroborated by at least two distinct agents — typically the spotter plus a confirming or refining adversary, or two independent spotters surfacing the same fix). Correctness/security issues with corroboration live here by default. When K ≥ 1, multiplicity ≥ 2 is automatic for any spotter proposal that any adversary confirmed or refined (the spotter's CHANGE and the adversary's CHANGE merge under 3c into one group with two distinct agents).
- **P1 — Do in this sweep.** Severity critical or high that didn't clear the P0 bar (a single agent surfaced it without independent corroboration), or severity medium. Clear, concrete, low-regression-risk changes.
- **P2 — Do if cheap.** Severity low or nit. Cosmetic/taste cleanups land here.
- **Skip.** Rejected by adversaries, or the adversary's replacement is also in the plan and supersedes it, or the proposal is vague after consolidation.

When K = 0 there are no adversaries to provide corroboration, so the only path to multiplicity ≥ 2 is two independent spotters surfacing the same fix — which is rare. Most K = 0 candidates therefore land at P1 or below; that's the trade-off for skipping adversarial review.

### 3e. Render the plan to the user

Before any execution, print the plan. Up to three sections, in order (the middle one is omitted when K = 0):

**Per-proposal overview** — a compact table, one row per *candidate that survived synthesis* (each spotter proposal, each adversary `replace` block whose CHANGE became its own candidate, and each adversary addition), grouped by tuple, showing what the candidate converged on:

| Tuple | Prop # | Target | Source | Original severity | Adv A verdict | Adv B verdict | … | Final status |
|-------|--------|--------|--------|-------------------|---------------|---------------|---|--------------|

`Source` is one of:
- `spotter` for the spotter's own proposals — `Original severity` is the spotter's severity, adv-verdict columns show each adversary's verdict for this proposal,
- `adv-A:rep` / `adv-B:rep` / … for adversary `replace` candidates — the letter identifies which adversary; that adversary becomes the sole agent vouching for this candidate, so `Original severity` carries the adversary's severity and the adv-verdict columns are blank,
- `adv-A:add` / `adv-B:add` / … for adversary additions — same blank-verdict-column rule and same severity convention.

The `Prop #` column is `1`, `2`, … for spotter rows; `A:rep1`, `A:rep2`, …, `B:rep1`, … for adversary replaces (the digit echoes the spotter `ORIGINAL_REF` that was replaced); `A:new1`, `A:new2`, `B:new1`, … for adversary additions (sequential within each adversary's contributions on that tuple). When a tuple has multiple candidates, render them as consecutive rows sharing the tuple number. `Final status` is one of: `→ P0`, `→ P1`, `→ P2`, `→ Skip`, or `→ merged with <tuple>.<prop#>` (when deduped into another row). When K = 0, drop the adv-verdict columns from the table header — there are no adversaries to display, and the only row source is `spotter`.

**Notable disagreements** — 3–5 bullets on the most interesting cases where adversaries changed the outcome materially (downgraded a proposal, replaced it with a better fix, rejected a confident spotter, surfaced something the spotter missed, or disagreed with each other when K ≥ 2). One line each. Omit when K = 0.

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

Execute **P0 and P1** improvements by default. Execute **P2** too unless the union of all `FILES` paths across P0+P1 candidates already exceeds ~10 distinct paths (count distinct file paths, not summed list lengths) — in which case skip the P2 fixers, mark the P2 items as deferred so they appear in the final report's "Obsolete / blocked / skipped / deferred" section, and invite the user to re-run. This keeps any single sweep's blast radius bounded.

### 4a. Group for parallel execution

For each improvement to execute, you already have its `FILES` list from the proposal. Build a list of `(improvement_id, files_touched)` tuples. Group into **waves** such that within one wave no two improvements share any file path, and any improvement whose files are unclear/unknown is placed in its own singleton wave (serial).

Algorithm (greedy, good enough):

1. Within each priority bucket, partition candidates into **peer-checked** (spotter proposals, plus adversary `confirm` / `refine` / `replace` reports — every one of which had at least one peer reading the target) and **adv-add** (`ORIGINAL_REF: new` additions, vouched for by exactly one agent). An `ORIGINAL_REF: new` candidate whose multiplicity after 3c dedup is ≥ 2 (multiple agents independently surfaced it) is promoted from the adv-add bucket to the peer-checked bucket — corroborating multiplicity is itself a form of peer review of the FILES list. (For sort purposes such a promoted candidate adopts the originating tuple of one of its surfacing agents, e.g. the lowest-numbered tuple it appears in.)
2. Sort peer-checked candidates by the lexicographic key `(tuple, spotter-proposal-number, candidate-kind, adversary-letter, addition-index)` ascending, where:
   - `tuple` = the originating tuple, or, for *promoted* adv-adds (multiplicity ≥ 2 after dedup), the lowest-numbered tuple that surfaced it (per step 1).
   - `spotter-proposal-number` = the `ORIGINAL_REF` for spotter proposals and adversary `replace` candidates; `∞` for promoted adv-adds (which have no spotter ref).
   - `candidate-kind` orders `spotter` (0) < `adv-replace` (1) < `promoted-adv-add` (2). This breaks the tie when a spotter proposal and an adversary `replace` of that same proposal share the same `(tuple, ORIGINAL_REF)` — the spotter goes first, then any `replace` candidates trail it.
   - `adversary-letter` and `addition-index` are only meaningful for `adv-replace` and `promoted-adv-add` rows; ignore them for spotter rows.
   
   Effect: siblings from the same tuple appear in the spotter's listed order; an adversary `replace` candidate immediately follows the spotter proposal it replaces; promoted adv-adds trail at the end of their adopted tuple. Sort the remaining (non-promoted) adv-add candidates by `(tuple, adversary-letter, addition-index-within-adversary)` so additions from the same adversary stay grouped and adversary letters break ties between tuples. Both sorts are stable; no other secondary heuristic (file count, severity within bucket, etc.) is applied.
3. **Phase A** — pack peer-checked candidates into waves: walk the sorted list and add each candidate to the earliest existing wave whose union of files has no intersection with this candidate's files. If no wave fits, open a new wave.
4. **Phase B (run after Phase A waves complete)** — pack adv-add candidates into trailing waves using the same rule. Adv-add fixers run *after* every peer-checked fixer in the same priority bucket has landed, so the working tree is at its most-edited state when they execute. This maximizes the chance an adv-add with a misjudged FILES list either lands cleanly or detects `STATUS: obsolete`, instead of clobbering an earlier peer-checked edit.
5. **Cross-bucket execution order**: process priority buckets sequentially — all P0 waves (Phase A then Phase B) run to completion before any P1 wave starts; all P1 waves before any P2 wave. The full execution chain is therefore P0-A → P0-B → P1-A → P1-B → P2-A → P2-B, skipping any empty (bucket, phase) slot. Wave numbers are continuous across the chain (so the user sees "wave 1, 2, 3, …" without resets between phases or buckets).
6. **Same-tuple siblings** (informational): two proposals from the same spotter that share any file are automatically separated by the disjointness check (different waves). Even when their FILES are disjoint, the numeric-order sort above guarantees PROPOSAL N never lands in a wave earlier than PROPOSAL N-1 from the same tuple — preserving the sequence the spotter intended. (When their FILES are disjoint they may end up co-packed in the *same* wave; this is fine because disjoint files cannot interfere, and the spotter prompt requires every proposal be robust to its siblings not having landed.)
7. **Brand-new files** (informational): a path in FILES that does not yet exist is treated like any other entry — two fixers both creating the *same* new path collide, two fixers creating different new paths do not. Fixers never touch files outside their declared FILES list, so any module registration (imports, `mod` declarations, index entries) that a new file needs is either already in FILES (fine), or will be handled by the Step 5 drift sweep after the fixer returns `STATUS: partial`.

**Be conservative.** If you are not *certain* two improvements touch disjoint files, put them in separate waves. A wrongly-parallelized pair of fixers can clobber each other's edits; the cost of a serial step is much lower than the cost of a merge conflict inside a subagent's diff. The same conservatism applies to adv-add proposals whose FILES list looks suspiciously narrow given the described change (e.g., FILES contains only one file when the described edit clearly requires touching imports, registrations, schemas, or callers in other files) — pack such proposals as singleton waves rather than co-pack them with each other.

### 4b. Dispatch fixer subagents per wave

For each wave, in a single message send one Agent call per improvement in the wave, all `subagent_type: general-purpose` and all `run_in_background: true`. Wait for the wave to finish before starting the next. (Parallelism within a wave; serial between waves.)

**Prompt template for each fixer** (each fixer dispatch also gets a freshly-generated unique SEED of 8 random English words — distinct from every other subagent in the run, no explanation given to the subagent):

> SEED: `<EIGHT RANDOM WORDS, SPACE-SEPARATED>`
>
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
> 1. Read the file(s) first to confirm the described state still matches reality. **Do not trust the line numbers in the proposal text** — earlier fixers in this same sweep may have already shifted lines, renamed symbols, or moved functions in adjacent code. Locate your target by symbol/intent, not by line number, and re-read fresh every time. If the described state has changed but the intent still clearly applies, adapt to the new reality. If the intent no longer applies at all (the function was removed, the section was deleted, the issue was already fixed), abort and report `STATUS: obsolete` with a one-line explanation.
> 2. Make the minimum edit that implements the change. Preserve surrounding style (indentation, quoting, naming conventions).
> 3. **Stay strictly within the declared FILES list.** You may only create or modify files that appear in that list. You may freely edit within those files — including same-file doc comments, inline examples, and in-file schemas — but do NOT edit any path outside that list, even if you believe it has drifted. Cross-file drift is reconciled in a dedicated sweep after you return. If you discover an edit outside your list is urgently needed, DO NOT make it; record it in `FOLLOWUPS` and set `STATUS: partial`. **When in doubt, prefer `partial` over `done`** — a `partial` status with an honest `FOLLOWUPS` list lets the drift sweep finish the job safely; a `done` status that silently under-edited is invisible to the drift sweep and ships broken state.
> 4. If the change is a code edit, also run any obviously-relevant local checks the repo uses (e.g. if there's a `cargo check` / `npm run typecheck` convention you can see from scripts, run it on the edited crate/package — do not run the full test suite). Do NOT attempt to fix unrelated pre-existing failures.
> 5. Do not commit. Leave edits in the working tree.
> 6. **STATUS legend.** Pick the value that honestly describes what happened. The downstream drift sweep treats every path in `FILES_EDITED` as the *new source of truth*, so be careful never to leave bad edits there.
>    - `done` — clean landing. Every in-scope edit applied as intended; the file(s) compile/parse/lint as well as they did before. `FILES_EDITED` lists what you wrote.
>    - `partial` — your in-scope edits landed cleanly, but more work is needed *outside your declared FILES list* (or sub-edits within scope that you intentionally deferred). `FILES_EDITED` lists what you wrote; `FOLLOWUPS` records what the drift sweep should still pick up. The edits in the working tree must be safe to ship as-is — they just don't finish the whole story.
>    - `obsolete` — the intent no longer applies (target was removed, the issue is already fixed, the change would be a no-op). You did not edit anything; `FILES_EDITED: none`.
>    - `blocked` — you could not complete the change *safely*. Examples: a declared file can't be read or written; a required tool is missing; the change applied but a local verification command (`cargo check`, `npm run typecheck`, etc.) flagged a real problem caused by your edit and you can't resolve it inside scope. **Revert any partial edits before reporting `blocked` so `FILES_EDITED: none`** — the working tree must end in a known-good state. (If your edit is independently sound but verification fails for an *unrelated* reason, that is `partial` with a `FOLLOWUPS` note, not `blocked`.)
>
>    Never use `done` for a partial or unsafe application — `partial` is the safe answer for incomplete-but-clean, `blocked` for couldn't-apply-safely.
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

**Before wave 1**, print a one-liner announcing the start of execution: `Starting execution: wave 1 (M improvements).` (No prior-wave status to report.)

**Between waves**, briefly (one or two lines) tell the user which improvements landed (`done` or `partial`), which were `obsolete` or `blocked`, and which wave is starting next. When the next wave is the first Phase B (adv-add) wave or the first wave of a new priority bucket, mention that explicitly — long sweeps are easier to follow when transitions are flagged. Keep it tight: one line of status + one line of "starting wave N (M improvements)".

---

## Step 5 — Drift-reconciliation sweep

After all waves complete (and if dry mode is off), run one final pass to catch drift the individual fixers were forbidden from touching. This step is mandatory whenever Step 4 edited at least one file. It is the ONLY place cross-file drift is allowed to be fixed.

### 5a. Decide whether to run

1. Collect the union of all `FILES_EDITED` paths across every fixer — this is the authoritative file set passed to the drift subagent. Separately, gather the text of every non-empty `FOLLOWUPS` field (these are free-text hints, not paths) — they are passed to the drift subagent alongside the authoritative file set as supplementary context.
2. If the authoritative file set is empty (every fixer was `obsolete`/`blocked` with no edits), skip this step and go to Step 6 — without any new source of truth there is nothing for the drift sweep to reconcile against, even if some `FOLLOWUPS` were reported. (Surface those `FOLLOWUPS` in Step 6's "Follow-ups worth considering" section instead.)
3. Otherwise, run the drift subagent below. Drift is bidirectional, so do NOT skip based on file types — an edited config, schema, or doc can just as easily obsolete code as the reverse.

### 5b. Dispatch the drift subagent

Dispatch one drift-reconciliation subagent with `subagent_type: general-purpose` (foreground is fine — it's a single agent and subsequent steps depend on its output; this dispatch also gets its own freshly-generated unique SEED of 8 random English words, no explanation given to the subagent):

> SEED: `<EIGHT RANDOM WORDS, SPACE-SEPARATED>`
>
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

### Obsolete / blocked / skipped / deferred

Bulleted list of anything the plan included but execution didn't complete. One line each. Three categories:

- **Obsolete / blocked**: a fixer was dispatched but couldn't apply the change. Quote the reason from the fixer's `SUMMARY`.
- **Deferred**: P2 items intentionally not dispatched because P0+P1 already exceeded the ~10-distinct-files threshold (per Step 4). These can be picked up by re-running `/improve`.
- **Malformed**: any subagent (spotter, adversary, fixer, drift) whose output couldn't be parsed and was treated as empty per the malformed-output rule. List one line per dropped subagent — bucket and role only, no reproduction of the garbled output.

Omit any category that has zero entries.

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
- **Spotters return 1+ proposals.** Each spotter's report is a list — one big change, several small ones, or a mix, sized to whatever the sampled target area actually warrants. Quality over quantity; don't pad to a number, don't stop at one if more genuinely deserve calling out. Each proposal must stand alone (a fixer is spawned per accepted proposal, and they may execute in parallel).
- **Adversaries review every proposal in the list.** One ADJUSTED block per spotter proposal (matched by `ORIGINAL_REF`), in numeric order, plus optional `ORIGINAL_REF: new` additions for issues the spotter missed in the same target area.
- **Dry mode is a flag** toggled by the literal token `dry` anywhere in the args. When on, stop after Step 3 (the plan is printed; no fixers run, no drift sweep runs). When off, execute the plan.
- **Adversaries read the target themselves** — they don't just argue from the original's text.
- **Spotters and adversaries don't edit files.** They only propose. Editing happens in Step 4 via dedicated fixer subagents.
- **Fixers stay in scope.** Each fixer implements exactly one improvement, no drive-by cleanup. Scope creep inside a fixer is the single most common way these sweeps go wrong.
- **Parallel only when provably disjoint.** If two improvements' file sets overlap, or if a file set is unknown/uncertain, the improvements go in different waves. Conservative wins; a wrong parallelization corrupts someone's edits.
- **Non-promoted adv-add proposals run last within their priority bucket.** A proposal vouched for by exactly one agent (an adversary addition with `ORIGINAL_REF: new` whose multiplicity stayed at 1 after 3c dedup) cannot be assumed to have a complete FILES list. Schedule all peer-checked candidates first, then dispatch these adv-add candidates as trailing waves so they execute against the most-edited working tree — they will land cleanly or detect `obsolete` rather than clobber an earlier landed edit. (An adv-add that gets *promoted* to peer-checked because two or more distinct agents independently surfaced it is treated as peer-checked from then on and runs in Phase A — corroborating multiplicity is itself a form of peer review of the FILES list.)
- **Same-tuple siblings run in numeric order.** Within a single spotter's tuple, PROPOSAL N is never scheduled in an earlier wave than PROPOSAL N-1. The spotter listed them in the order they made sense to apply; preserve it so later siblings see earlier siblings' edits and can either adapt or report `obsolete` cleanly.
- **Fixers locate by symbol, not by line.** Earlier fixers in the same sweep may have shifted lines, renamed symbols, or moved code. Every fixer re-reads its declared FILES fresh and locates its target by symbol/intent, not by the line numbers the proposal text mentioned.
- **When in doubt, prefer `STATUS: partial`.** A partial status with honest `FOLLOWUPS` lets the drift sweep finish the job; a `done` status that silently under-edited ships broken state.
- **Drift reconciliation is mandatory.** Whenever Step 4 edited at least one file, run the Step 5 drift sweep. Drift is bidirectional — do not skip just because "only docs changed" or "only code changed." Fixers are forbidden from touching undeclared files, so Step 5 is the only place cross-file reconciliation happens.
- **SEED salt.** Every subagent prompt — every spotter, every adversary, every fixer, and the drift dispatch — must begin with a `SEED:` line containing 8 freshly-generated random English words, space-separated. Each subagent across the entire run must receive a *different* set of 8 words; do not reuse seeds across spotters, across adversaries, across fixers, between any of the four agent types, or between sweeps. Do not explain the field's purpose to subagents and do not reference it elsewhere in the prompt.
- **Don't pre-fetch** code for spotters or adversaries. They do their own reading; that's part of the sample.
- **Treat malformed subagent output as empty.** A probabilistic sweep with many subagents will occasionally see one return output that doesn't match the declared format, errors out, or never completes. Treat that subagent's contribution as if it produced nothing for that step:
  - Spotter: treat as `PROPOSALS: none` for that tuple (still spawn its K adversaries — they may emit `ORIGINAL_REF: new` additions).
  - Adversary: treat as no ADJUSTED blocks (the spotter's proposals lose one reviewer for verdict-counting; the strict-majority threshold remains `> half of K`).
  - Fixer: treat as `STATUS: blocked, FILES_EDITED: none` and assume nothing was edited (do NOT trust any partial edits the working tree may show from a crashed fixer — verify by reading the declared FILES if you need certainty).
  - Drift subagent: treat as `FILES_UPDATED: none, UNRESOLVED: drift sweep failed`.
  
  Never block the rest of the sweep on a single malformed reply. Surface each dropped subagent under the final report's "Malformed" bucket (one line: `<role> for <target/improvement>`) so the user sees what was lost.
- **Don't commit.** All edits stay in the working tree for the user to review, test, and commit.
