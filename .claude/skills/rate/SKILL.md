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
2. If any token equals `improve` (case-insensitive), set **improve mode = on** and remove that token.
3. Parse the remaining tokens positionally as N then K. Tokens that are missing, non-numeric, or not an integer (e.g. `8.5`, `foo`) use the default; tokens beyond the second are ignored. Then clamp N ≥ 1 and K ≥ 0 (so `0` and negatives parse as valid integers and get clamped afterwards — and K = 0 is legal, meaning no adversarial pass, just raw rater ratings).

The `improve` token may appear before, between, or after the numeric args — `/rate improve 8`, `/rate 8 improve`, and `/rate 8 2 improve` are all equivalent to N=8, K=2, improve=on.

## Flow at a glance

1. Spawn **N rater subagents** in parallel (background). Each picks one random target and reports a rating + thesis.
2. As each rater report arrives, immediately spawn **K adversarial subagents** in parallel (background) that independently produce adjusted versions of that report. (If K = 0, skip this step.)
3. When all N originals have all K adversarial adjustments back (= N originals + N·K adversarial adjustments = **N·(K+1) total ratings, in N tuples of size K+1**), write one **final synthesis report** with an overall rating.

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

**Prompt template for each rater** (customize the category hint per agent):

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

- In your **next turn**, send a single message with **K Agent calls** (parallel, all `run_in_background: true`) for that report.
- Continue handling other rater notifications as they come in. Do NOT batch — interleave eagerly.

Each adversary gets the same prompt independently:

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
> TARGET: <same target, possibly refined>
> RATING: <your adjusted float, 0.00-1.00, two decimals>
> THESIS: <2-4 sentences. What you found that the original missed or got wrong. Why your number is more accurate.>
> ```
>
> The rating may move up, down, or stay. Stay only if after genuine adversarial scrutiny you still agree — no rubber-stamping.

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

Once every original has all its adversarial adjustments back (N tuples of size K+1, N·(K+1) ratings total), write the final report directly to the user (no file). Include:

### Per-target table

A table with one row per original target. Each row has a `#` index, a `Target` label (one-line description plus file path), the `Original` rating, one rating column per adversary (Adv A, Adv B, …, up to K adversary columns), and a `Tuple mean` column. When K = 0, there are no adversary columns and the tuple mean equals the original rating.

| # | Target | Original | Adv A | Adv B | … | Tuple mean |
|---|--------|----------|-------|-------|---|------------|

### Notable disagreements

The 3-5 most interesting cases where adversaries moved the rating significantly (≥ 0.15 in either direction), or — when K ≥ 2 — where adversaries on the same target disagreed strongly with each other. One line each, explaining what the disagreement was about. Omit this section entirely when K = 0 (no adversaries, no disagreements).

### Final overall rating

Compute as the **mean of the N tuple means** (so each target weighs equally regardless of how many adversaries pile on). Round to 2 decimals.

> **PROBABILISTIC RATING: X.XX / 1.00**

### Synthesis (3-6 sentences)

What this random sample suggests about the project: where it's strong, where it's weak, what kinds of issues showed up repeatedly, and the standing caveat: *this is a random sample, not a complete review — re-run /rate for a different sample.*

### Actionable improvements (improve mode only)

Include this section **only when improve mode is on**; otherwise omit it entirely.

Collect every IMPROVEMENTS bullet from all N·(K+1) subagent reports (skip `IMPROVEMENTS: none`). If no bullets remain after the skip, render the section as a single line — "_No actionable improvements surfaced in this sample._" — and stop. Otherwise deduplicate: when multiple agents surfaced the same fix for the same file/line, merge them into one bullet and note the multiplicity as a confidence signal. Then group and prioritize:

- **High priority** — fixes multiple agents converged on, or fixes sourced from a target where original-vs-adversary ratings differ by ≥ 0.15 (the disagreement signals a contentious area worth acting on), or correctness/security issues (not style).
- **Medium priority** — fixes surfaced by one agent that target a specific file with a concrete change.
- **Low priority** — stylistic/cosmetic fixes, nice-to-haves.

Render each group as a bulleted list. Each bullet must cite the file path (and line/range when the source provided it), state the concrete change, and — when the same fix was flagged by multiple agents — include a short parenthetical like `(flagged by 3 agents)`. Keep bullets tight; do not re-explain context already covered in the per-target table.

---

## Hard rules

- **N raters, parallel, background.** One message, N Agent calls. Default N = 16; override from first positional argument.
- **K adversaries per rater, eager.** Spawn them the moment the rater returns, not in a batch at the end. Default K = 2; override from second positional argument. K = 0 skips the adversarial pass entirely.
- **Improve mode is a flag** toggled by the literal token `improve` anywhere in the args. When on, every rater and adversary prompt must include the IMPROVEMENTS extension block, and the final report must include an "Actionable improvements" section synthesized from those blocks. When off, the IMPROVEMENTS block is never requested and the section is omitted.
- **Adversaries read the target themselves** — they don't just argue from the original's text.
- **Rating math:** tuple mean per target = arithmetic mean of the K+1 ratings for that target (original rater plus its K adversaries; when K = 0 the tuple is just the original). Final rating = mean of the N tuple means. Two decimals throughout. Improve mode does not affect the math.
- **No file writes.** The final report goes in chat.
- **Don't pre-fetch** code for raters or adversaries. They do their own reading; that's part of the sample.
- **Don't normalize or smooth** the spread. Honest variance is the signal.
