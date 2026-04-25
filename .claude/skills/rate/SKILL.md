---
name: rate
description: |
  Probabilistic / sampled quality rating of the current codebase, project, or app. Spawns N parallel rater subagents (default 16) that each sample one random aspect (a function, struct, pattern, doc section, project META, etc.) and score it 0.00-1.00 with a thesis. Each rater report is then adversarially challenged by K more subagents (default 2) that produce their own adjusted reports. Finally synthesizes one overall project rating.

  Optional args: `/rate` uses defaults; `/rate N` overrides rater count (e.g. `/rate 8`); `/rate N K` overrides both rater count and adversaries-per-rater (e.g. `/rate 32 4`). Add the literal token `improve` anywhere in the args (e.g. `/rate improve`, `/rate 8 improve`, `/rate 32 4 improve`) to also have every subagent report actionable improvement suggestions for whatever they flagged, and to receive a synthesized "Actionable improvements" section in the final report.

  TRIGGER on /rate, AND on any natural-language request that asks for an overall judgment of project / codebase / repo quality without specifying a method. Examples that MUST trigger this skill:
    - "how good is my codebase / project / repo / code?"
    - "what do you think of this project / codebase?"
    - "rate my project / code / repo"
    - "give me a quality score / rating / grade for this codebase"
    - "is this codebase any good?"
    - "review the whole project" (when no specific file/PR is named)
    - "what's your opinion of this codebase?"
    - "how would you score this repo?"
    - any "how good", "how bad", "rate", "score", "grade", "judge", "opinion", "thoughts on" phrasing aimed at the codebase as a whole

  DO NOT trigger when: the user names a specific file/function/PR to review (use normal review flow), asks a focused code question, asks about a single bug, or invokes /review or /security-review explicitly.

  When inferring, just call the skill — do not ask the user to confirm. The skill itself is the answer to "how good is it?"
---

# /rate — Probabilistic Project Rating

Random-sample quality assessment. The point is **not** completeness. You are deliberately rating a scattered handful of aspects to "get a taste" of the project, then letting adversaries challenge each rating before synthesis. Variance and surprise are features, not bugs.

## Arguments

The skill accepts up to two optional positional numeric arguments (N, K) plus one optional flag token (`improve`):

- `/rate` → defaults: **N = 16** raters, **K = 2** adversaries per rater, improve mode **off**
- `/rate N` → **N** raters, still **K = 2** adversaries per rater (e.g. `/rate 8` → 8 raters × 2 adv = 24 ratings)
- `/rate N K` → **N** raters, **K** adversaries each (e.g. `/rate 32 4` → 32 raters × 4 adv = 160 ratings)
- `/rate improve` → defaults + **improve mode on** (subagents suggest fixes, final report includes "Actionable improvements")
- `/rate N improve` or `/rate N K improve` → same N/K override as above, plus improve mode on

Parsing rules:

1. Split the argument string on whitespace.
2. If any token equals `improve` (case-insensitive), set **improve mode = on** and remove every such token (multiple `improve` tokens collapse to one flag).
3. Parse the remaining tokens positionally as N then K. Tokens that are missing, non-numeric, or not an integer (e.g. `8.5`, `foo`) use the default; tokens beyond the second are ignored. Then clamp N ≥ 1 and K ≥ 0 (so `0` and negatives parse as valid integers and get clamped afterwards — and K = 0 is legal, meaning no adversarial pass, just raw rater ratings).

Very large N or K is allowed, but each rater plus its K adversaries are dispatched as background subagents through the harness. If N or N·K is large enough that a single message exceeds tool-call limits, fall back to dispatching the raters in successive same-message batches (still all background) and apply the eager-adversary rule per rater as their notifications arrive. Never serialize a rater behind another rater just to reduce concurrency — the sample is meant to be parallel.

The `improve` token may appear before, between, or after the numeric args — `/rate improve 8`, `/rate 8 improve`, and `/rate 8 2 improve` are all equivalent to N=8, K=2, improve=on.

## Flow at a glance

1. Spawn **N rater subagents** in parallel (background). Each picks one random target and reports a rating + thesis.
2. As each rater report arrives, immediately spawn **K adversarial subagents** in parallel (background) that independently produce adjusted versions of that report. (If K = 0, skip this step.)
3. When every dispatched subagent has either returned or been resolved (any rater whose report was unparseable is dropped immediately along with its tuple, and adversaries are only dispatched for parseable raters), write one **final synthesis report** with an overall rating. In the all-parseable case this is N originals + N·K adversaries = **N·(K+1) total ratings, in N tuples of size K+1**.

Do not wait for all raters before launching adversaries. Eager spawning is a hard requirement whenever K ≥ 1 (when K = 0 there are no adversaries, so this rule is vacuous).

---

## Step 1 — Launch N raters in parallel

In a single message, call the Agent tool **N times** with `subagent_type: general-purpose` and `run_in_background: true`. Give each rater a different **category hint** so the sample spreads across the project. Suggested pool — if N ≤ pool size, sample N distinct hints; if N > pool size, use each hint once and fill the remainder with the wildcard:

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
- a wildcard the rater finds interesting

**Prompt template for each rater** (customize the category hint and SEED per agent — every subagent across the entire run, raters and adversaries alike, must receive a *unique* SEED of 8 freshly-generated random English words; do not reuse seeds across subagents and do not explain the field to them):

> SEED: `<EIGHT RANDOM WORDS, SPACE-SEPARATED>`
>
> You are a rater in a probabilistic project review. The repo is at the working directory.
>
> **Your category:** `<CATEGORY HINT>`. Find ONE concrete target in this category by exploring the repo. Pick something specific — not "the codebase" but e.g. "the `reconcile_index` function in src/store/index.rs", or "the error type hierarchy in src/error.rs", or "the section of README.md describing the storage layout".
>
> Read the actual code/file. Form an opinion. Then return EXACTLY this format and nothing else:
>
> ```
> TARGET: <one-line description of what you rated, with file path>
> RATING: <float between 0.00 and 1.00, two decimals>
> THESIS: <2-4 sentences. Concrete observations. Why this rating, not 0.1 higher or lower. Cite specific lines or behavior.>
> ```
>
> Calibration (be honest, not diplomatic):
> - 0.90+ — excellent, would point to as exemplary
> - 0.75 — solidly good, minor nits
> - 0.60 — acceptable, clear room for improvement
> - 0.45 — mediocre, noticeable problems
> - 0.30 — bad, real issues a maintainer should care about
> - 0.15 or below — broken, harmful, or absent where it shouldn't be
>
> Avoid clustering near 0.7. Spread your ratings honestly.

**If improve mode is on,** append the following block to the rater prompt above. It overrides the base prompt's "nothing else" constraint: the full expected output now has four fields, in order — TARGET, RATING, THESIS, IMPROVEMENTS.

> Add an IMPROVEMENTS block immediately after THESIS (this extends, not replaces, the earlier format):
>
> ```
> IMPROVEMENTS:
> - <specific, actionable fix — file:line + concrete change>
> - <another fix, if applicable>
> ```
>
> List 1–5 concrete, actionable improvements total, covering the issues you surfaced in your thesis (one bullet per distinct fix, not one bullet per issue). Each bullet should name the file (and line/range when useful) and state the exact change (rename, delete, split, add a test, replace X with Y, etc.) — not "consider improving error handling" but "replace `c.is_alphanumeric()` at validation.rs:24 with `c.is_ascii_alphanumeric()` so Cyrillic lookalikes are rejected at parse time". If the target is already strong and you genuinely found nothing actionable, write `IMPROVEMENTS: none` and nothing else in that block.

## Step 2 — Spawn adversaries eagerly as each rater returns

Skip this step entirely if K = 0 — go directly to Step 3 and treat each rater's rating as its final rating.

You will receive a notification each time a background rater finishes. The instant a rater's report arrives:

- **First, parse it.** Locate the rater's `TARGET:` / `RATING:` / `THESIS:` lines using the lenient-parsing rule (see "Subagent failures and lenient parsing" in the Hard rules). If the report is unparseable (no extractable TARGET/RATING/THESIS, or RATING is non-numeric / out of [0.00, 1.00]), **do NOT spawn adversaries for it** — the tuple is dropped immediately and contributes nothing to the math or to the improvements list. Note the drop and move on.
- If parseable, in your **next turn** send a single message with **K Agent calls** (parallel, all `run_in_background: true`) for that report.
- If multiple rater notifications arrive together in the same input, parse each one and fire adversary calls for the parseable ones in that same message (K parallel calls per surviving rater) — co-occurrence is not deferral.
- Never wait for additional raters before spawning adversaries for a rater that has already returned and parsed cleanly.

Each adversary gets the same prompt independently (each adversary also receives its own freshly-generated unique SEED of 8 random English words — distinct from every other rater and adversary in the run, no explanation given to the subagent):

> SEED: `<EIGHT RANDOM WORDS, SPACE-SEPARATED>`
>
> You are an adversarial reviewer in a probabilistic project rating. Another rater produced this report:
>
> ```
> <PASTE ORIGINAL REPORT VERBATIM>
> ```
>
> Your job: **poke holes**. Read the same target yourself. Argue the rating is too high OR too low. Look for:
> - context the original rater missed (callers, tests, history, related files)
> - bias in their framing (gave benefit of the doubt? was overly harsh?)
> - hidden virtues or hidden problems they didn't surface
> - whether their thesis actually supports the number they assigned
>
> Then produce YOUR OWN adjusted report in EXACTLY the same format:
>
> ```
> TARGET: <same artifact as the original — you may correct an obvious typo, file path, or one-line description, but you must rate the same underlying file/function/section the original named. Do NOT swap to a different artifact even if you'd rather rate something else nearby.>
> RATING: <your adjusted float, 0.00-1.00, two decimals>
> THESIS: <2-4 sentences. What you found that the original missed or got wrong, OR — if you kept the rating — why it survives scrutiny. Either way: why your number is the right number, citing concrete evidence from your own re-read.>
> ```
>
> The rating may move up, down, or stay. Stay only if after genuine adversarial scrutiny you still agree — no rubber-stamping.
>
> **If you genuinely cannot locate the artifact** the original named (it appears to have been renamed, deleted, or hallucinated, and no obvious correction recovers it), say so in your THESIS and write `RATING: n/a` instead of a number. This is the only valid non-numeric rating.

**If improve mode is on,** append the same IMPROVEMENTS extension to the adversary prompt. The adversary's full expected output now has four fields, in order — TARGET, RATING, THESIS, IMPROVEMENTS.

> Add an IMPROVEMENTS block immediately after THESIS (this extends, not replaces, the earlier format):
>
> ```
> IMPROVEMENTS:
> - <specific, actionable fix — file:line + concrete change>
> - <another fix, if applicable>
> ```
>
> List 1–5 fixes *you* would recommend based on your own re-read — they may overlap with the original rater's list (that's useful signal), contradict it, or add fixes the original missed. Each bullet must name the file (and line/range when useful) and state the exact change, not a vague aspiration. If after adversarial scrutiny you genuinely think the target needs no improvements, write `IMPROVEMENTS: none`.

Adversaries run independently; when K ≥ 2 they may disagree with each other, which is fine.

## Step 3 — Final synthesis

Once every dispatched subagent has either returned (rater + its K adversaries, where applicable) or had its tuple dropped for parse failure, write the final report directly to the user (no file). Include:

### Per-target table

A table with one row per **surviving** original target (any tuple dropped for parse failure does not appear in the table — it is summarized in the `_Excluded:_` line below the table instead). Each row has a `#` index, a `Target` label (one-line description plus file path), the `Original` rating, one rating column per adversary (Adv A, Adv B, …, up to K adversary columns), and a `Tuple mean` column. Empty cells (e.g., a single dropped adversary) are rendered as `—`. When K = 0, there are no adversary columns and the tuple mean equals the original rating.

| # | Target | Original | Adv A | Adv B | … | Tuple mean |
|---|--------|----------|-------|-------|---|------------|

### Notable disagreements

The 3-5 most interesting cases where an adversary moved the rating significantly (|adversary − original| ≥ 0.15) or — when K ≥ 2 — where two adversaries on the same target disagreed strongly with each other (max − min ≥ 0.15 within the tuple). One line each, explaining what the disagreement was about.

Ranking when more than 5 tuples qualify: rank by **disagreement magnitude** = max(|adv − original|) across surviving adversaries in the tuple, then — when K ≥ 2 — take the larger of that and (max − min) across all surviving ratings in the tuple. Show the top 5 by that magnitude, ties broken by lower tuple index (#) first. If only 1, 2, 3, or 4 tuples meet the threshold, show exactly those (don't pad). Render "_No notable disagreements in this sample._" only when **zero** tuples meet either threshold. Omit the section entirely when K = 0.

### Final overall rating

Compute as the **mean of the surviving tuple means** — one mean per tuple that has at least one parseable rating left after exclusions, so each surviving target weighs equally regardless of how many adversaries pile on. Round to 2 decimals.

If **zero tuples survive** (every rater produced unparseable output, which should be very rare), skip the math and the synthesis paragraph and emit only:

> _All N raters produced unparseable output — no rating computed. Re-run /rate to try again._

Otherwise:

> **PROBABILISTIC RATING: X.XX / 1.00**

When the surviving tuple count M is less than N, append a one-line note immediately under the rating: `_Computed from M of N tuples; N − M excluded due to parse failure (see table)._`

### Synthesis (3-6 sentences)

What this random sample suggests about the project: where it's strong, where it's weak, what kinds of issues showed up repeatedly, and the standing caveat: *this is a random sample, not a complete review — re-run /rate for a different sample.*

### Actionable improvements (improve mode only)

Include this section **only when improve mode is on**; otherwise omit it entirely.

Collect every IMPROVEMENTS bullet from every **surviving** subagent report — i.e., reports that were not excluded for parse failure under the rule in "Subagent failures and lenient parsing". If a rater was dropped (its whole tuple excluded), its IMPROVEMENTS bullets are dropped too; if an adversary was dropped (its rating excluded from its tuple's mean), its IMPROVEMENTS bullets are likewise dropped.

Skip any report whose **entire IMPROVEMENTS block** is a literal "none". Concretely, take the block — everything from the `IMPROVEMENTS:` label through the end of the report (or the next recognized field label, if the subagent kept writing past it) — strip surrounding whitespace, and skip iff that stripped text matches the regex `^IMPROVEMENTS:\s*none\.?\s*$` (case-insensitive). A report whose IMPROVEMENTS section starts with the word "none" but then continues with prose or bullets does NOT match — extract its bullets normally.

Bullet extraction from a non-skipped IMPROVEMENTS block: each non-empty line after `IMPROVEMENTS:` whose first non-whitespace character is `-`, `*`, or `•`, **or** which begins with a numbered/lettered list marker (`1.`, `1)`, `a.`, etc.), is treated as one bullet. Strip the marker and surrounding whitespace before deduplication. Ignore blank lines and continuation prose between bullets.

If no bullets remain after exclusions and skips, render the section as a single line — "_No actionable improvements surfaced in this sample._" — and stop.

Otherwise deduplicate: when multiple agents surfaced the same fix for the same file/line, merge them into one bullet and note the multiplicity as a confidence signal. If two fixes target the same file/line but propose mutually exclusive changes, keep both bullets and tag them `(conflicting suggestions — operator must choose)`. Then group and prioritize:

- **High priority** — fixes multiple agents converged on (across tuples or within a tuple), or correctness/security issues (not style); additionally, any fix sourced from a tuple that meets the disagreement threshold defined in "Notable disagreements" above — i.e., when K ≥ 1, |adversary − original| ≥ 0.15 for any adversary in the tuple; when K ≥ 2, additionally max − min ≥ 0.15 across all surviving ratings in the tuple. This applies to every threshold-meeting tuple, regardless of whether it made the displayed top 3-5. The disagreement signals a contentious area worth acting on. When K = 0 there is no adversary signal, so only the "multiple agents converged" and "correctness/security" sub-rules apply.
- **Medium priority** — fixes surfaced by one agent that target a specific file with a concrete change.
- **Low priority** — stylistic/cosmetic fixes, nice-to-haves.

Render each group as a bulleted list. Each bullet must cite the file path (and line/range when the source provided it), state the concrete change, and — when the same fix was flagged by multiple agents — include a short parenthetical like `(flagged by 3 agents)`. Keep bullets tight; do not re-explain context already covered in the per-target table.

---

## Hard rules

- **N raters, parallel, background.** One message, N Agent calls. Default N = 16; override from first positional argument.
- **K adversaries per rater, eager.** The moment a rater returns and parses cleanly, spawn its K adversaries — not in a batch at the end. (If the rater report is unparseable, spawn no adversaries; the tuple drops immediately.) Default K = 2; override from second positional argument. K = 0 skips the adversarial pass entirely.
- **Improve mode is a flag** toggled by the literal token `improve` anywhere in the args. When on, every rater and adversary prompt must include the IMPROVEMENTS extension block, and the final report must include an "Actionable improvements" section synthesized from those blocks. When off, the IMPROVEMENTS block is never requested and the section is omitted.
- **Adversaries read the target themselves** — they don't just argue from the original's text.
- **Rating math:** tuple mean per target = arithmetic mean of the surviving ratings for that target (original rater plus its K adversaries — minus any excluded for parse failure; when K = 0 the tuple is just the original, and a tuple with zero surviving ratings is dropped entirely). Final rating = arithmetic mean of the surviving tuple means. Compute with full float precision; round only the values shown in the per-target table and the final PROBABILISTIC RATING line (2 decimals, standard half-up rounding). If zero tuples survive, do not compute a final rating — emit the fallback line instead (see Step 3). Improve mode does not affect the math.
- **No file writes.** The final report goes in chat.
- **Don't pre-fetch** code for raters or adversaries. They do their own reading; that's part of the sample.
- **Don't normalize or smooth** the spread. Honest variance is the signal.
- **SEED salt.** Every subagent prompt — every rater and every adversary — must begin with a `SEED:` line containing 8 freshly-generated random English words, space-separated. The 8-word *combination* must be unique across every subagent dispatched by **this single /rate invocation** (uniqueness is scoped to the current run, not globally across past or concurrent invocations) — no two subagents in the run share the same set. Individual words may recur across different sets; only the full set has to differ. Do not explain the field's purpose to subagents and do not reference it elsewhere in the prompt.
- **State tracking.** Maintain a clear mental model of which raters have returned, which adversaries are still pending per rater, and which tuples are complete. The final synthesis (Step 3) fires the moment the last incomplete tuple becomes complete — no earlier, no later. For runs with N·(K+1) ≥ 24, use TodoWrite to track tuple completion explicitly.
- **Subagent failures and lenient parsing.** Subagents may prepend prose before the required block — locate the `TARGET:` / `RATING:` / `THESIS:` (and `IMPROVEMENTS:` when applicable) lines and parse from them; strict format-only output is not required. Match field labels case-insensitively and ignore trailing punctuation (e.g. `Rating: 0.7`, `IMPROVEMENTS: None.`, `improvements: none` are all acceptable). If a field label appears multiple times in one report (multiple `TARGET:`, `RATING:`, `THESIS:`, or `IMPROVEMENTS:` lines), use the **first occurrence** of each and treat any later ones as part of surrounding prose. RATING also accepts values without a leading zero or with extra precision (`.7`, `0.756`) and clamps display rounding to 2 decimals; computations use full precision. If a subagent's response lacks a parseable `RATING:` line with a numeric in [0.00, 1.00], or omits `TARGET:` / `THESIS:`, exclude that report entirely — its rating drops from the math AND its IMPROVEMENTS bullets (if any) drop from the improvements list. A **missing or unparseable `IMPROVEMENTS:` block in improve mode is NOT a parse failure** — keep the report (its rating still counts) and treat it as having no improvement bullets. An adversary that **clearly rated a different artifact** than the original's TARGET (e.g., the adversary's TARGET line names a different file or function and the change is not a typo/path correction permitted by the Target stability rule) is also treated as a parse failure: drop its rating from the tuple mean and its IMPROVEMENTS bullets from the improvements list. A failed rater drops the entire tuple (and the orchestrator does NOT spawn its K adversaries — see Step 2); a failed adversary just drops its own rating from its tuple's mean. Note any drops in a one-line `_Excluded: <count and reason>_` row immediately under the per-target table when any occurred.
- **Target stability.** Adversaries may correct an obvious typo, file path, or one-line description in the rater's TARGET, but must rate the same underlying artifact (file, function, section). They are forbidden from swapping to a different artifact even if they'd rather rate something else nearby. If an adversary genuinely cannot locate the artifact (renamed, deleted, hallucinated, and no obvious correction recovers it), the adversary prompt instructs it to say so in its THESIS and return `RATING: n/a` — the orchestrator treats that as a parse failure under the rule above and drops the adversary from the tuple mean (along with its IMPROVEMENTS bullets, if any).
