---
name: improve
description: |
  Probabilistic / sampled improvement sweep of the current codebase, project, or app. Begins by asking the user two upfront scoping questions — a minimum severity floor and the areas of focus — both defaulting to "no filter" so accepting defaults reproduces the skill's prior behavior. Then spawns N parallel spotter subagents (default 16) that each sample one random small cluster (a function plus its closest collaborators, a struct plus its impls, 3-6 related source files, a doc section plus the code it references, a recurring pattern, project META, etc.), read it thoroughly end-to-end, and propose 3+ concrete improvements with rationale — typically 5-10 across the cluster, a mix of larger structural changes and smaller fixes. Each spotter report is then adversarially challenged by K more subagents (default 2) that re-read the same target deeply and produce their own adjusted lists — confirming, refining, replacing, or rejecting each proposal, and surfacing real issues the spotter missed as additions. Finally synthesizes one deduplicated, prioritized improvement list and EXECUTES every accepted improvement: most edits are applied **directly by the main agent inline** (the inline bar is intentionally wide — up to ~3 tightly-coupled files, ~60-line budget, private-symbol signature changes allowed — so fixer-subagent overhead is reserved for changes that genuinely fan out), while cross-file ripples, public-API contract changes, and codebase-wide caller fan-out are dispatched to **fixer subagents** — running them in parallel when their file sets are provably disjoint, serially otherwise. Improvements may target any file (code, docs, configs, schemas, CI, etc.); after execution a drift-reconciliation sweep fixes anything on either side that no longer matches (code ↔ docs, code ↔ configs, etc.).

  Optional args: `/improve` uses defaults; `/improve N` overrides spotter count (e.g. `/improve 8`); `/improve N K` overrides both spotter count and adversaries-per-spotter (e.g. `/improve 32 4`). Add the literal token `dry` anywhere in the args (e.g. `/improve dry`, `/improve 8 dry`, `/improve 32 4 dry`) to stop after synthesis — produce the prioritized plan but do NOT execute any fixer subagents.

  TRIGGER on /improve, AND on any natural-language request that asks for open-ended improvement of the project / codebase / repo without specifying a target. Examples that MUST trigger this skill:
    - "improve my codebase / project / repo / code"
    - "find things to fix in this project"
    - "clean up / polish / tighten this codebase"
    - "what should I fix?"
    - "sweep the repo for improvements"
    - any "improve", "polish", "tighten", "clean up", "fix things", "what needs work" phrasing aimed at the codebase as a whole

  DO NOT trigger when: the user names a specific file/function/PR to fix (edit directly), asks about a single concrete bug, invokes /review or /security-review, or asks only for a *rating* (use /rate).

  When inferring, just call the skill — do not ask the user to confirm.
---

# /improve — Probabilistic Project Improvement Sweep

Random-sample improvement pass. The point is **not** completeness — sample a scattered handful of aspects, surface a mixed bag of improvements, let adversaries challenge each, then execute the survivors. Variance is a feature; re-run for a different sample.

Action-oriented sibling of `/rate`. Where `/rate` judges, `/improve` fixes.

## Arguments

Two optional positional integers (N, K) plus optional `dry` flag:

- `/improve` → N = 16, K = 2, dry off
- `/improve N` → N spotters, K = 2 each
- `/improve N K` → N spotters, K each
- `dry` token (anywhere in args) → produce plan, skip execution

Parsing: split on whitespace; remove every `dry` token (case-insensitive) and set the flag; parse remaining tokens positionally as N then K. Non-integer or missing tokens use defaults; tokens past the second are ignored. Clamp N ≥ 1, K ≥ 0. K = 0 = no adversarial pass.

## Flow

0. Ask the user (one `AskUserQuestion`) for severity floor + areas of focus. Defaults are no-op.
1. Spawn N spotters in parallel (background).
2. As each spotter returns, eagerly spawn K adversaries for it (background, parallel).
3. Synthesize one deduplicated, prioritized plan (P0/P1/P2/Skip).
4. Execute: classify each accepted improvement as **inline** (main agent applies directly) or **subagent** (fixer dispatched). Skipped if dry mode.
5. Run drift-reconciliation sweep (one subagent) to fix anything that referenced edited files. Skipped if dry mode or no edits landed.
6. Final report.

Eager adversary spawning is mandatory whenever K ≥ 1 and the spotter returned a parseable report. Don't wait to batch.

---

## Step 0 — Confirm scope with user

Before any subagents, ask via a single `AskUserQuestion`:

1. **Minimum severity floor**: `nit` (default, no filter), `low`, `medium`, `high`, `critical`. Inclusive — `medium` keeps medium/high/critical, drops low/nit. Spotters receive the floor and pre-filter their own proposals, so most below-floor candidates never enter the pipeline. Anything that still slips through (e.g. an adversary downgrades a spotter proposal below the floor, or surfaces a below-floor addition) shows in the per-proposal overview as `→ Skip (below floor)` and never executes.
2. **Areas to focus on**: `all` (default, full pool) or free-text mapped loosely to entries in the Step 1 category-hint pool (e.g. `security, performance, tests`, or `the storage subsystem and its tests`). Always retain the wildcard entry as fallback.

If non-interactive (no user attached), fall back to defaults silently. After answers, echo one line: `_Scope: severity ≥ medium, areas = [security, tests, performance]; sweeping now…_`.

Run Step 0 even when `dry` is on — the questions scope what gets synthesized, not just what executes.

---

## Step 1 — Launch N spotters in parallel

One message, N Agent calls, `subagent_type: general-purpose`, `run_in_background: true`. Each spotter gets a different **category hint** and the **severity floor** chosen in Step 0 (every spotter gets the same floor).

**Build the available pool** by filtering the list below using the user's `areas` answer (`all` → full pool; subset → keep entries whose meaning overlaps; always retain the wildcard fallback). If N ≤ pool size, sample N distinct hints; else use each once and fill the rest with the wildcard.

Pool:
- a function or method plus its closest collaborators (1-3 callers, helpers it calls, or its tests)
- a struct / class / type / enum and the impls / methods / constructors that operate on it
- a small group of 2-4 related source files (a module with its sub-files, a file plus its tests, sibling files in the same subsystem)
- a recurring pattern (error handling, logging, validation, retries, ID generation, …)
- a style/formatting choice (naming, comments, line length, import ordering)
- a structural decision (folder layout, module boundaries, dependency graph)
- an architectural approach (concurrency model, state management, data flow)
- a section of a doc file (README, CLAUDE.md, design doc, ADR) plus the code or configs it references
- project META: `.gitignore`, CI config, Dockerfile, build scripts, release process
- dependency choices (what's pulled in, version pinning, alternatives)
- test coverage / quality for some specific area
- API/CLI surface ergonomics
- handling of a specific edge case or failure mode
- security posture of one specific surface
- performance characteristics of one specific path
- a wildcard the spotter finds interesting

Every subagent in the run (spotter, adversary, fixer, drift) must receive a *unique* SEED of 8 freshly-generated random English words. Don't explain SEED to the subagent.

**Spotter prompt:**

> SEED: `<EIGHT RANDOM WORDS>`
>
> You are a spotter in a probabilistic project improvement sweep. The repo is at the working directory.
>
> **Your category:** `<CATEGORY HINT>`. Find one concrete target area in this category. Aim for a **focused cluster** you can read carefully end-to-end — not "the codebase" and not just one isolated function/struct, but a coherent slice with enough context to spot real issues. Examples: "the `reconcile_index` function plus the 2-3 helpers it calls and its tests in src/store/index.rs"; "the `CartItem` struct, the `Cart` impls that operate on it, and the validation helpers they call"; "the README's storage-layout section plus the `src/store/mod.rs` and `src/store/index.rs` it points to"; "the 4 files that make up the pricing subsystem plus their tests". Roughly 2-5 functions, 1 struct + its impls + closely related types, or 3-6 related files is the right size — broad enough to surface many distinct improvements, narrow enough that you can still read every line.
>
> **Severity floor: `<SEVERITY FLOOR>`.** Only propose improvements at this severity or higher (severity order: `nit < low < medium < high < critical`). **Silently drop anything below the floor** — don't list it, don't mention it, don't surface near-misses or "you might also consider…" items. Below-floor noise wastes downstream budget. Keep searching at-or-above the floor; don't reach below it to fill space.
>
> **If your assigned cluster is dry, pivot — don't return empty.** After reading carefully, if you find nothing at-or-above the floor worth changing in the initial cluster, either (a) **expand outward** — its callers, its dependencies, neighboring files in the same module/subsystem — while staying within the spirit of your category, or (b) **walk to a different cluster** that still fits your category. Reflect the pivot in `TARGET_AREA` (e.g. "expanded from `reconcile_index` to surrounding `src/store/index.rs` after the original cluster was clean at the medium floor"). Two pivots is the norm; three is the cap. `PROPOSALS: none` is only allowed after honest pivots still turn up nothing — and should be rare, since most clusters yield several at-or-above-floor improvements once read carefully.
>
> **Read deeply before you start writing proposals.** Walk every file in the cluster top-to-bottom — every function, every type, every test, every comment, every error path. Trace data flow through the cluster: where do inputs come from, what invariants are assumed, what happens on failure, what edge cases are unhandled. Note any obvious smells (duplicated logic, dead code, racy state, brittle parsing, missing tests, misleading docs) AND less-obvious ones (semantic mismatches between callers and callees, contracts that aren't enforced, comments that lie, error types that erase information). Open referenced files when the cluster points to them. **Skim-reading misses the issues that matter** — a 30-second skim and a 5-minute careful read produce wildly different proposal lists, and the careful read is the whole point.
>
> Then propose **3 to ~12 concrete, actionable improvements** for whatever cluster you ultimately settled on — a mix of larger structural changes and smaller fixes, sized to whatever you find. **Quality first, but be thorough.** A careful read of a real cluster almost always surfaces 5+ real issues at-or-above a reasonable floor; if you only have 1-2 proposals, you probably haven't read deeply enough — re-read or expand the cluster before reporting. **Don't pad** with manufactured nits — but don't stop at the first shallow pass either. Proposals can target different items within the cluster (e.g. two fixes in function A, three in struct B, one in the doc that references both, one in a test). List proposals in the order you'd want them applied; each must be robust to its siblings succeeding, failing, being deemed obsolete, or running concurrently.
>
> Return EXACTLY this format and nothing else:
>
> ```
> TARGET_AREA: <one-line description with file path(s)>
>
> PROPOSAL 1
> SEVERITY: <critical | high | medium | low | nit>
> CATEGORY: <correctness | security | performance | clarity | tests | docs | style | deps | build | other>
> FILES: <comma-separated list of every file path this proposal would create or edit, including required companions (imports, mod declarations, schemas, fixtures, generated artifacts). Always list paths; never "none". An incomplete FILES list forces the fixer to report `partial` or `blocked`.>
> CHANGE: <2-5 sentences. Concrete change. Name the file and line range. State the exact edit (rename, delete, split, add a test, replace X with Y, etc.) — not "consider improving error handling" but "replace `c.is_alphanumeric()` at validation.rs:24 with `c.is_ascii_alphanumeric()` so Cyrillic lookalikes are rejected at parse time".>
> RATIONALE: <1-3 sentences. Why this matters.>
>
> PROPOSAL 2
> ...
> ```
>
> Number from 1 with contiguous integers; one blank line between PROPOSAL blocks; every field mandatory.
>
> Severity calibration:
> - **critical** — bug that corrupts data, leaks secrets, or crashes on normal input
> - **high** — real correctness/security/perf problem
> - **medium** — clear defect or obvious quality win
> - **low** — minor nit, readability, small cleanup
> - **nit** — cosmetic / taste-level
>
> If you genuinely find nothing worth changing **after pivoting at least twice and reading exhaustively**, return `TARGET_AREA: <what you ultimately looked at, including any pivots>` followed by `PROPOSALS: none` and nothing else. This should be rare. Do not invent improvements to fill a quota, and do not surface below-floor items just to have something to say.

## Step 2 — Spawn adversaries eagerly

Skip entirely if K = 0.

The instant a spotter returns:

- **Malformed** (no TARGET_AREA): treat as `PROPOSALS: none`, skip its adversaries.
- Otherwise: in your **next turn**, send one message with K Agent calls (parallel, background) for that report. If multiple spotters complete in the same turn, pack their adversary calls together — that's still eager.

**Adversary labeling:** within a tuple, the K adversaries are `A`, `B`, `C`, … in **dispatch order** (the order of Agent calls in the outgoing message). Fixed at dispatch, not return order. K > 26 → `AA`, `AB`, … (lexicographic).

**Adversary prompt** (each gets its own unique SEED):

> SEED: `<EIGHT RANDOM WORDS>`
>
> You are an adversarial reviewer. Another spotter produced this report; one or more proposals:
>
> ```
> <PASTE THE SPOTTER'S REPORT — verbatim if all blocks parsed, or with garbled PROPOSAL blocks dropped and survivors renumbered 1..M contiguously per the malformed-output rule. Preserve TARGET_AREA exactly.>
> ```
>
> Read the same target yourself **thoroughly** — every file the spotter named, every line of every function in scope, every test, every comment. Trace data flow independently. Don't lean on the spotter's framing; build your own mental model first, then compare it against their proposals. For EACH numbered proposal, decide: (a) correct and worth doing, (b) wrong-priority (severity mis-set), (c) subtly incorrect — would regress something, (d) redundant, or (e) replaceable by a better fix. **You are also expected to surface real issues the spotter missed as additions** — a careful independent read of a real cluster commonly turns up 1-3 things the spotter overlooked. Aim for thorough coverage of the cluster, not just verdicts on the existing list.
>
> Look for: missed callers/tests/invariants/related files, behaviors that would regress, smaller/cheaper fixes, mis-set severity/category, inaccurate FILES lists, overlapping proposals that should collapse, **important issues missed entirely** — including issues in parts of the cluster the spotter didn't examine closely (a function's error path, a test's assertions, a comment that lies, an unchecked invariant). Spend roughly as much reading-time as the spotter did before writing your verdicts.
>
> Emit one ADJUSTED block per spotter proposal (numeric order, `ORIGINAL_REF: <N>`), plus optional ADJUSTED blocks for additions (`ORIGINAL_REF: new`):
>
> ```
> TARGET_AREA: <same target, possibly refined>
>
> ADJUSTED 1
> ORIGINAL_REF: 1
> VERDICT: <confirm | refine | replace | reject>
> SEVERITY: <adjusted>
> CATEGORY: <adjusted>
> FILES: <adjusted; for reject, "none">
> CHANGE: <adjusted concrete change; for reject, "none">
> RATIONALE: <1-3 sentences.>
>
> ADJUSTED 2
> ORIGINAL_REF: new
> VERDICT: addition
> ...
> ```
>
> VERDICT calibration:
> - **confirm** — original is right; same fix.
> - **refine** — same issue, different fix.
> - **replace** — real issue, wrong fix; propose better.
> - **reject** — no real issue or fix would regress; CHANGE = `none`.
> - **addition** — only with `ORIGINAL_REF: new`.
>
> Rules:
> - One ADJUSTED block per spotter proposal in numeric order. No skipping — vote `confirm` if you have nothing to add.
> - **Additions are encouraged when your independent read surfaces real issues the spotter missed.** A thorough adversary commonly contributes 1-3 additions; zero additions is fine if you genuinely found nothing new, but it should be the result of careful reading, not the default.
> - If spotter was `PROPOSALS: none`: rule above is vacuous. You may emit `ORIGINAL_REF: new` blocks (and you should, if your independent read disagrees with their "nothing here" verdict); or return `TARGET_AREA: <same>` + `ADJUSTMENTS: none`.
> - No rubber-stamping.

---

## Step 3 — Synthesis

Once every tuple has resolved (every spotter returned or was malformed; every spawned adversary returned or was malformed):

### 3a. Collect

Gather every CHANGE from every PROPOSAL block and ADJUSTED block. Drop `PROPOSALS: none`, `ADJUSTMENTS: none`, and `CHANGE: none`. What remains is the raw candidate set; each candidate is one proposal.

### 3b. Resolve adversary verdicts per proposal

For each spotter proposal, look at the K adversaries' ADJUSTED blocks referencing it. Verdict counting reads raw adversary outputs (including rejects whose CHANGE was filtered to "none" in 3a), not the post-3a candidate set.

- **Strict-majority reject** (> half of K voted `reject`; threshold = `floor(K/2) + 1` — so K = 1 needs 1 reject, K = 2 needs 2, K = 3 needs ≥ 2, K = 4 needs ≥ 3): drop the spotter's CHANGE plus every `confirm`/`refine`/`replace` CHANGE for that `ORIGINAL_REF`. Adversary additions (`ORIGINAL_REF: new`) on the same tuple are independent and survive. The dropped change may still resurface from a different tuple — 3c handles that.
- Otherwise the proposal survives:
  - `confirm` — adversary's block joins the spotter's into one logical candidate (multiplicity counts both). Spotter wording kept.
  - `refine` — joins into one candidate (multiplicity counts both). Adversary wording supersedes. With mixed `confirm`+`refine`, keep refined wording and surface the disagreement.
  - `replace` — adversary's CHANGE enters as a separate candidate (multiplicity-1 starter). 3c may dedup it cross-tuple.
- **Adversary additions**: each enters as its own candidate, multiplicity 1.
- **K = 0**: every spotter proposal passes through with multiplicity 1.

### 3c. Deduplicate across proposals

Group surviving candidates that target **the same underlying change** — same file(s) + same semantic edit, even if worded differently.

- **Automatic intra-tuple grouping**: a spotter `PROPOSAL N` and any adversary `ADJUSTED` block with `ORIGINAL_REF: N` from the same tuple whose verdict is `confirm` or `refine` group automatically. Cross-tuple grouping and matching `replace`/adv-add CHANGEs against unrelated candidates require judgment.
- Prefer the most specific wording (refine-supersedes-confirm already applied within each automatic intra-tuple group).
- **Merge FILES by union, not replacement.** Drop exact duplicates; do not deduplicate paths that only differ in casing or trailing slashes (let the fixer normalize).
- **Multiplicity** = number of distinct source agents contributing a CHANGE. The same agent's two overlapping proposals count once.
- **Severity conflicts**: take the most severe. **Category conflicts**: majority vote among non-rejected agents; tiebreak `security > correctness > performance > tests > deps > build > docs > clarity > style > other`.
- A candidate rejected on one tuple but confirmed on another survives — only contributing agents count toward multiplicity; the rejecting tuple's reject doesn't reduce it.

### 3d. Prioritize

Assign each surviving candidate to:

- **P0 — Do now.** Severity critical/high AND multiplicity ≥ 2. (When K ≥ 1, any spotter proposal an adversary confirmed/refined automatically has multiplicity ≥ 2.)
- **P1 — Do in this sweep.** Severity critical/high without corroboration, or severity medium.
- **P2 — Do if cheap.** Severity low/nit.
- **Skip.** Any of:
  - Resolved severity is **strictly below the user-chosen severity floor** (severity order: `nit < low < medium < high < critical`). Checked first; overrides everything else. Surface in overview as `→ Skip (below floor)`.
  - Too vague after consolidation to write a fixer prompt for.
  - An adversary `replace` candidate for the same area is also in the plan and clearly supersedes this one.
  - The candidate is a *spotter* proposal that K ≥ 1 adversaries reviewed and every reviewing adversary voted `replace` or `reject` (no `confirm`/`refine`). The replace candidates proceed normally; the spotter's own CHANGE goes to Skip. (Doesn't apply when K = 0 or when every adversary was malformed.)

(Strict-majority adversary rejects don't reach 3d — they were dropped at 3b.)

When K = 0, the only path to multiplicity ≥ 2 is two independent spotters surfacing the same fix — rare. Most K = 0 candidates therefore land at P1 or below.

### 3e. Render the plan

Print before any execution. Three sections (middle one omitted when K = 0):

**Per-proposal overview** — table grouped by tuple, one row per surviving candidate:

| Tuple | Prop # | Target | Source | Original severity | Adv A verdict | Adv B verdict | … | Final status |
|-------|--------|--------|--------|-------------------|---------------|---------------|---|--------------|

`Source`: `spotter` (adv-verdict columns show each adversary's verdict for this proposal; severity is the spotter's) | `adv-A:rep` / `adv-B:rep` / … (replace candidates; adv-verdict columns blank — no adversary voted on the replace itself; severity is the adversary's) | `adv-A:add` / `adv-B:add` / … (additions; same blank-verdict / adversary-severity rule).

`Prop #`: `1`, `2`, … for spotter rows; `A:rep1`, `A:rep2`, …, `B:rep1`, … for adv replaces (digit echoes spotter `ORIGINAL_REF`); `A:new1`, `A:new2`, `B:new1`, … for adv additions (sequential within each adversary's contributions).

`Final status`: `→ P0` | `→ P1` | `→ P2` | `→ Skip` | `→ merged with <tuple>.<prop#>`. When K = 0, drop adv-verdict columns.

**Notable disagreements** — up to ~5 bullets on the most interesting cases (downgrades, replaces, rejects of confident spotters, surfaced misses, adversary disagreements when K ≥ 2). Omit if none or if K = 0.

**The plan** — three lists (P0, P1, P2). Each bullet: severity, category, file(s), one-line description, `(flagged by M agents)` parenthetical when M ≥ 2. Skip empty groups. Example:

> **P0 — Do now**
> - **[high / correctness]** [src/validation.rs:24](src/validation.rs#L24) — replace `c.is_alphanumeric()` with `c.is_ascii_alphanumeric()` so Cyrillic lookalikes are rejected at parse time _(flagged by 3 agents)_

If no candidates survived synthesis OR every survivor went to Skip: render the per-proposal overview anyway (so the user sees the work spotters and adversaries did), then `_No actionable improvements surfaced in this sample — re-run /improve for a different sample._`, followed by a **Malformed** sub-bucket listing every dropped subagent (one line each: `<role> for <target/improvement>`; omit if none). Then stop.

**If dry mode is on, stop here.** Tell the user `/improve` without `dry` would execute this plan, BUT re-running `/improve` produces a different sample — to apply *this* plan, they should act on it now.

---

## Step 4 — Execute

Execute **P0 + P1** by default. Execute **P2** too unless the union of all `FILES` paths across **P0+P1+P2** exceeds **10 distinct paths in the union** (count distinct paths after lowercase + strip-trailing-slash normalization, not summed list lengths) — in which case skip P2 fixers, mark them deferred (Step 6 "Not landed → Deferred"), invite re-run. P0+P1 still execute regardless of the union size; only P2 is deferred. The threshold is on the *total* would-be blast radius — a small P0+P1 with a sprawling P2 still defers.

### 4a. Classify: inline vs subagent

A fixer subagent dispatch costs 5–15K tokens of overhead (system prompt, tool defs, working-dir context, prompt template, fresh re-reads) before it makes a single edit. **Most fixer work in practice is small** — typo fixes, single-site renames, short pattern replacements, small new test cases, localized refactors, private-symbol signature tweaks — and dispatching a subagent for that kind of work is pure waste. The inline bar is intentionally wide; only escalate to a subagent when the change *genuinely* fans out across the codebase.

- **Inline** (main agent applies via Read + Edit) — eligible if ALL:
  1. `FILES` contains **up to ~3 paths**, where each path is either tightly coupled to the others or self-contained enough to edit without losing track. Tightly-coupled examples: a function + its co-located test file + a small doc/changelog touch; a struct + its impl file + a single fixture; a renamed private symbol + the few call sites that are already named in FILES. The bar is whether you can hold all listed files in your head simultaneously and edit them coherently — not whether they share a directory or language.
  2. The CHANGE stays inside the declared FILES — no caller chasing beyond what FILES already lists, no codebase-wide grep, no dependency-graph reasoning beyond what the FILES list already implies.
  3. **Total edit budget ≤ ~60 lines across all listed files**, with up to **~30 lines in any single new structural block** (a new ~30-line function/struct/test case is fine; spreading edits across 2-3 files within the ~60-line ceiling is fine; a brand-new ~100-line module is subagent territory).
  4. Observable-behavior changes are **inline-eligible when the affected symbol is private to the listed files** — file-local helper, in-module struct, internal trait method, non-exported function. **Public/exported signature or contract changes are subagent**, because callers across the codebase need updating and the FILES list can't enumerate them all. Behavior-preserving body edits (refactor, simplify, perf-tune) are inline regardless of visibility.

- **Subagent** — anything else: cross-file ripple beyond what FILES enumerates, codebase-wide caller fan-out, schema/type changes that propagate to many readers, structural refactors that span modules, brand-new files that need registration outside the declared FILES, **deletion of a public/exported symbol** (callers across the codebase need updating), and **any change to a public function's signature or observable contract**.

**Private/file-local symbol changes are inline** — including signature changes, deletions, and contract tweaks, as long as every caller is already inside the declared FILES list. **Public/exported deletion or signature change is subagent** — the spotter's "required companions" rule lets the fixer's FILES list include callers, which inline can't atomically locate when they're scattered across the codebase.

**Safety net.** An inline attempt that turns out to need cross-file work aborts cleanly: stop, leave the working tree clean for what was written, mark `partial` with a `FOLLOWUPS` note. Drift sweep (Step 5) picks up the rest. The safety net catches *unexpected* ripple the main agent notices mid-edit; it does NOT catch silent semantic changes the agent doesn't detect, which is why item 4 above is a hard up-front filter, not a runtime check.

**When in doubt, prefer inline.** A `partial` from an over-eager inline attempt is cheap — the drift sweep finishes the job. An unnecessary fixer-subagent dispatch is not.

### 4b. Pack subagent improvements into waves

Build `(improvement_id, files_touched)` for each subagent-classified improvement. Group into waves where no two improvements within a wave share a file (path normalized: lowercase, trailing slashes stripped — for wave-packing only; 3c keeps casing variants distinct in the merged FILES list itself). Inline improvements are not packed.

Algorithm:

1. Within each priority bucket, partition into **peer-checked** (spotter proposals + adversary `confirm`/`refine`/`replace`) and **adv-add** (`ORIGINAL_REF: new`). Adv-adds with multiplicity ≥ 2 after 3c are *promoted* to peer-checked (corroborating multiplicity is itself peer review of the FILES list). A promoted candidate's identity for sort: lowest-numbered tuple it appears in; within that tuple, lowest adversary letter; within that adversary, lowest addition-index.
2. Sort peer-checked by `(tuple, spotter-proposal-number, candidate-kind, adversary-letter, addition-index)` ascending where:
   - `spotter-proposal-number` = `ORIGINAL_REF` for spotter and adv-replace; `∞` for promoted adv-add.
   - `candidate-kind`: `spotter` (0) < `adv-replace` (1) < `promoted-adv-add` (2). Spotter goes before any replace of the same proposal.
   - `adversary-letter` is the dispatch-order letter; `addition-index` is the 1-based position of an `ORIGINAL_REF: new` block within that one adversary's report. Both meaningful only for adv-replace and promoted-adv-add.
   - Sort non-promoted adv-adds by `(tuple, adversary-letter, addition-index)`.
   - **Cross-merged candidates** (3c group spans tuples or kinds): if any contributor is a *spotter*, inherit the lowest-key spotter's identity (so it slots alongside that spotter's siblings); otherwise lowest contributor key. Both rules are deterministic and override the promoted-candidate identity rule when both apply.
   - Both sorts are stable; no other tiebreak (severity, file count, etc.) is applied.
3. **Phase A** — pack peer-checked: walk sorted list, place each into the earliest existing wave **within current bucket+phase** whose file union is disjoint; else open a new wave.
4. **Phase B** — pack adv-adds the same way, in trailing waves *after* Phase A completes. Phase B candidates never join a Phase A wave even when files would be disjoint.
5. **Cross-bucket order**: P0-inline → P0-A → P0-B → P1-inline → P1-A → P1-B → P2-inline → P2-A → P2-B (skip empty slots). Wave numbers continuous for display only; the packer never considers waves outside its current slot.
6. Same-tuple siblings auto-separate when their FILES overlap; when disjoint, the sort guarantees PROPOSAL N never lands in an earlier wave than PROPOSAL N-1.
7. Brand-new files: a new path in FILES is treated like any other — same path collides; different new paths don't. Module registrations a new file needs that aren't in FILES are handled by the Step 5 drift sweep after the fixer returns `partial`.

**Be conservative.** When uncertain about disjointness, separate waves. Singleton-wave any adv-add whose FILES list looks suspiciously narrow given the described edit.

### 4c. Apply inline improvements

Per priority bucket, before any subagent wave: walk inline improvements in the same sort order as 4b. For each:

1. Read the file fresh. **Don't trust line numbers in the proposal** — earlier inline edits may have shifted lines. Locate by symbol/intent.
2. Apply via Edit (Write only for a brand-new file from scratch). Minimum edit; preserve style; don't refactor adjacent code.
3. Stay strictly within the declared FILES list. If the edit ripples beyond what was declared (a caller in another file, a schema referenced in a third file, a structural refactor), STOP, leave the working tree clean for what you did write, mark `partial` with a `FOLLOWUPS` note, continue. Don't promote to subagent mid-execution and don't go grepping the codebase for callers — that's the drift sweep's job.
4. If the target no longer exists, record `obsolete`, continue without editing.
5. If the file changed in a way that makes the edit unsafe, revert any partial edit so the working tree is back to known-good, record `blocked`, continue.
6. Optionally run an obviously-relevant local check (`cargo check` / `npm run typecheck`) on the edited package. Don't run full test suite; don't fix unrelated pre-existing failures.
7. Don't commit.

Record the same `(STATUS, FILES_EDITED, SUMMARY, FOLLOWUPS)` quadruple a fixer subagent would produce, plus a `Mode: inline` tag — feeds Step 5 and Step 6 on equal footing with subagent fixer output. (Subagent-applied improvements get tagged `Mode: subagent` from their dispatch in 4d.)

### 4d. Dispatch fixer subagents

Per wave (subagent path), one message with one Agent call per improvement, all `subagent_type: general-purpose`, `run_in_background: true`. Wait for the wave to finish before the next.

**Fixer prompt** (each gets a unique SEED):

> SEED: `<EIGHT RANDOM WORDS>`
>
> You are a fixer subagent. Implement EXACTLY this improvement and nothing else. Don't expand scope, don't refactor nearby lines.
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
> 1. Read the file(s) first. **Don't trust line numbers in the proposal text** — earlier fixers may have shifted lines or renamed symbols. Locate by symbol/intent. If state changed but intent applies, adapt. If intent no longer applies (function removed, issue already fixed), abort with `STATUS: obsolete`.
> 2. Make the minimum edit. Preserve style.
> 3. **Stay strictly within declared FILES.** May modify any path in the list (including in-file doc comments, examples, schemas) but NOT outside it. Cross-file drift is reconciled in a dedicated sweep after you return. If you discover an outside-list edit is urgently needed, record it in `FOLLOWUPS` and use `STATUS: partial`. **When in doubt, prefer `partial` over `done`** — `partial` lets the drift sweep finish the job; a `done` that under-edited ships broken state.
> 4. For code edits, run an obviously-relevant local check (`cargo check` / `npm run typecheck`) on the edited package — not the full test suite. Don't fix unrelated pre-existing failures.
> 5. Don't commit.
> 6. **STATUS legend:**
>    - `done` — clean landing; every in-scope edit applied.
>    - `partial` — in-scope edits landed cleanly but more work needed outside scope (or sub-edits intentionally deferred). Tree must be safe to ship as-is.
>    - `obsolete` — intent no longer applies; nothing edited; `FILES_EDITED: none`.
>    - `blocked` — couldn't apply safely (file unreadable, tool missing, verification flagged a real problem caused by your edit). **Revert any partial edits before reporting `blocked`** so `FILES_EDITED: none`. (If your edit is sound but verification fails for an *unrelated* reason, that's `partial` with a `FOLLOWUPS` note.)
>
> Return EXACTLY:
>
> ```
> STATUS: <done | partial | obsolete | blocked>
> FILES_EDITED: <comma-separated paths, or "none"; MUST be a subset of declared FILES>
> SUMMARY: <1-3 sentences>
> FOLLOWUPS: <comma-separated notes on drift or adjacent issues you noticed but did NOT act on, or "none">
> ```

### 4e. Between phases / waves

If the entire run has zero work after deferral (e.g. P0+P1 empty, P2 deferred): say `_Nothing to execute — every accepted improvement was deferred. See "Not landed → Deferred" below._`, jump to Step 6.

Before each bucket's inline phase: `Starting <bucket> inline (M edits).`. After: one-line summary, e.g. `Inline P0: 4 done, 1 obsolete.`.

Before wave 1 of a bucket's subagent execution: `Starting execution: wave 1 (M improvements).`.

Between waves: one or two lines on what landed (`done`/`partial`)/`obsolete`/`blocked` + which wave is next. Flag transitions explicitly when a Phase B starts, when a new bucket starts, or when a new bucket's inline phase starts.

---

## Step 5 — Drift-reconciliation sweep

Mandatory whenever Step 4 edited at least one file. Skipped if dry mode is on.

### 5a. Decide whether to run

1. Authoritative file set = union of all `FILES_EDITED` across executed improvements (inline + subagent), *after* applying the malformed-output rule (intersect each fixer's `FILES_EDITED` against its declared `FILES`, drop out-of-scope, zero out internally-inconsistent reports). Out-of-scope paths a rogue fixer wrote are surfaced in Step 6 "Malformed", not passed to drift. Separately collect every non-empty `FOLLOWUPS` (free text, hints).
2. If the authoritative file set is empty (every executed improvement was `obsolete`/`blocked` with no edits), skip Step 5; surface any `FOLLOWUPS` in Step 6 "Follow-ups worth considering".
3. Otherwise, dispatch the drift subagent. Don't skip based on file types — drift is bidirectional.

### 5b. Dispatch

One drift subagent (foreground, `subagent_type: general-purpose`, unique SEED):

> SEED: `<EIGHT RANDOM WORDS>`
>
> You are a drift-reconciliation subagent. These files were just edited and represent the **new source of truth**:
>
> ```
> <LIST OF FILES_EDITED with each fixer's SUMMARY>
> ```
>
> Follow-up drift hints fixers reported but did not act on:
>
> ```
> <FOLLOWUPS, or "none">
> ```
>
> Find every *other* file in the repo whose content is now inconsistent with these edits, and update it. Drift is bidirectional:
>
> - Code edited → docs/examples/fixtures/configs/schemas/CLI reference/changelog/tests that name old symbols may need updating.
> - Docs/configs/schemas/CI/fixtures edited → code that reads those keys, implements those flags, parses those schemas, or fulfills those documented contracts may need updating.
> - Renames/moves/removed flags may leave stale references in any file — grep for old names broadly.
>
> Rules:
> 1. Listed files are authoritative — don't modify further. Only edit files that disagree with them.
> 2. Only fix drift directly caused by the listed edits. Don't fix unrelated drift, refactor, or invent docs.
> 3. Ambiguous direction → trust the edited side (chosen deliberately by the improvement sweep). Genuinely unsure → leave both, flag in `UNRESOLVED`.
> 4. If a fixer's edit looks wrong in light of what you find, DO NOT revert — flag in `UNRESOLVED`.
> 5. Don't commit.
>
> Return EXACTLY:
>
> ```
> FILES_UPDATED: <comma-separated paths, or "none">
> CHANGES: <one short bullet per file>
> UNRESOLVED: <comma-separated notes, or "none">
> ```

---

## Step 6 — Final report

In chat, no file:

### Executed

Table: one row per improvement that returned a parseable status (inline + subagent). Columns: `#`, `Priority`, `Mode` (`inline`/`subagent`), `Target`, `Status`, `Files edited`. Malformed subagent fixers are NOT here — they go to "Not landed → Malformed". Inline edits cannot be malformed in the subagent sense (the main agent self-reports); a `blocked` inline outcome lands here normally.

### Not landed

Sub-buckets (omit empty):

- **Obsolete / blocked**: an executed improvement (inline or subagent) couldn't apply. Quote `SUMMARY`.
- **Deferred**: P2 items skipped because the 10-distinct-paths threshold was exceeded. Pick up by re-running `/improve`.
- **Malformed**: any subagent (spotter, adversary, fixer, drift) treated as empty per the malformed-output rule. One line each: `<role> for <target/improvement>`.

(3d-`Skip` candidates appear only in the per-proposal overview from 3e — not re-listed here.)

### Drift reconciled

Bulleted list of files touched by Step 5 with short notes. State explicitly if the sweep ran but found nothing, or if it itself was malformed (distinguish "swept and found nothing" from "sweep failed" so the user knows to review cross-file consistency manually). Omit section if Step 5 didn't run.

### Unresolved

The drift subagent's `UNRESOLVED` field. Omit when empty.

### Follow-ups worth considering

Aggregate every non-empty fixer `FOLLOWUPS` the drift sweep didn't already resolve. Deduplicate. Bullets. Hints for next run, not executed in this sweep.

### Closing note

`_This was a random sample, not a complete pass — re-run /improve for a different sample._`

---

## Cross-cutting invariants

- **Step 0 always runs first** in interactive runs (including dry mode). Non-interactive → defaults silently.
- **N spotters parallel + K adversaries eager.** One outgoing message for the spotter batch; one message per spotter-completion turn for its adversaries (or one message packing several spotters' adversary calls if multiple completed in the same turn). Don't batch-wait.
- **SEED on every subagent prompt** — 8 unique random English words; don't reuse across subagents in a run; don't explain to subagent.
- **Spotters and adversaries never edit files** — only propose. Editing happens in Step 4.
- **Spotters return 3+ proposals (typically 5-10)**, each one a stand-alone unit (a fixer is spawned per accepted proposal — inline or subagent, with most accepted proposals executing inline). Quality first, but be thorough; a careful end-to-end read of a real cluster almost always surfaces several improvements.
- **Spotters read deeply before proposing** — every line, every error path, every test, every comment in the cluster, plus referenced files. Skim-reads waste the sample.
- **Spotters pre-filter at the severity floor** — they receive the floor and silently drop below-floor candidates rather than reporting them. If their initially-assigned cluster turns up nothing at-or-above the floor, they pivot (expand outward to callers/dependencies/neighboring files in the same module, or walk to a different cluster in the same category) before falling back to `PROPOSALS: none`. Two pivots is the norm; three is the cap. `PROPOSALS: none` should be rare.
- **Adversaries review every spotter proposal** — one ADJUSTED block per proposal in numeric order (`ORIGINAL_REF: <N>`), no skipping, plus `ORIGINAL_REF: new` additions whenever their independent read surfaces real issues the spotter missed.
- **Adversaries read the target themselves, thoroughly** — they don't argue from the spotter's text alone, and they spend roughly as much reading-time as the spotter did. Additions are encouraged when warranted; rubber-stamping is forbidden.
- **Dry mode** suppresses everything past Step 3: no inline edits, no fixer subagents, no drift sweep, no Step 6 execution table.
- **Inline by default; subagent only when the change genuinely fans out.** A subagent dispatch costs 5–15K tokens of overhead — most fixer work is small, so route everything that fits within ~3 tightly-coupled declared files and a ~60-line edit budget inline, including private/file-local signature and contract changes. Subagent only when the edit ripples across multiple unrelated files, changes a *public/exported* contract whose callers aren't on the FILES list, or needs a codebase-wide caller search. Inline aborts to `partial` if it turns out to fan out, so the safety net is built in. **When uncertain, prefer inline** — the partial-fallback path is cheap; an unnecessary subagent dispatch isn't.
- **Fixers stay in scope** (subagent and inline). No drive-by cleanup. Re-read fresh; locate by symbol, not line numbers.
- **Parallel only when files are provably disjoint.** Conservative wins — wrongly-parallelized fixers clobber each other.
- **Non-promoted adv-adds run last in their bucket** (Phase B). Single-voucher FILES lists can't be trusted.
- **Same-tuple siblings preserve numeric order within their classification path.** Inline-classified siblings run in order during the inline phase; subagent-classified siblings preserve order across waves. When siblings span paths, the inline phase always runs first per the bucket-phase order — acceptable because the spotter prompt requires every proposal to be robust to its siblings not having landed.
- **When in doubt, fixer reports `partial`.** A `done` that under-edited ships broken state.
- **Drift sweep is mandatory** when Step 4 edited any file. Bidirectional. Only place cross-file drift is allowed to be fixed.
- **Don't pre-fetch** code for spotters/adversaries. Their own reading is part of the sample.
- **Don't commit.** Edits stay in the working tree for the user to review.

### Malformed-output handling

A probabilistic sweep occasionally sees a subagent return garbled or absent output. Treat each malformed contribution as empty (don't block the rest of the sweep) and surface it under Step 6 "Malformed":

- **Spotter** missing TARGET_AREA: treat as `PROPOSALS: none`, skip its K adversaries (they have nothing to anchor on). A spotter with TARGET_AREA but partially garbled PROPOSAL blocks: drop *only* the broken proposals, renumber survivors 1..M contiguously, let adversaries spawn against the readable list.
- **Adversary** entirely unparseable: no ADJUSTED blocks contributed; the spotter's proposals lose one reviewer for verdict-counting. Strict-majority denominator stays the original K (so a malformed adversary makes rejects *harder* — conservative direction). Orphaned ADJUSTED block (`ORIGINAL_REF` references a non-existent proposal): drop only that block. Partial-coverage adversary (TARGET_AREA + at least one parseable ADJUSTED block, missing/unparseable for some proposals): preserve the parseable blocks; missing ones act like a missing reviewer for that single proposal; valid additions still count. Don't list under "Malformed" unless *every* ADJUSTED block was unusable.
- **Fixer**: treat as `STATUS: blocked, FILES_EDITED: none`; assume nothing was edited (verify by reading declared FILES if needed). Also malformed if internally inconsistent: `done` with `FILES_EDITED: none`; `obsolete` with non-empty `FILES_EDITED`; `blocked` with non-empty `FILES_EDITED`; `FILES_EDITED` containing any path **outside the declared FILES list**. For out-of-scope paths specifically, intersect `FILES_EDITED` against declared `FILES` before passing to the drift sweep, and surface a one-liner in "Malformed" naming the rogue paths so the user can review and revert in the working tree.
- **Drift subagent**: treat as `FILES_UPDATED: none, UNRESOLVED: drift sweep failed`.
