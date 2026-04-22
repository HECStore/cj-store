# Mission

This is a systematic quality pass over the entire codebase, function by function. The goal is not to add things — it is to reach the right state: every comment that remains earns its place, every meaningful behavior has a test that would catch a regression, and every log line gives an operator exactly what they need to understand what the system is doing or what went wrong. Proceed file by file: batch-read all functions in a file in parallel, apply all three reviews, then move to the next. When this pass is complete, the codebase should be in a state where no comment is noise, no test is misleading, and no operator is flying blind.

## Subagent strategy

Subagents must be used to the maximum extent possible throughout this pass. The main agent's job is coordination and final judgment — not raw reading and searching.

**Parallelize aggressively.** When starting a file, spawn one subagent per function simultaneously — do not read functions one at a time. Each subagent reads whatever it needs for its function (source, test file, call sites) and reports back. Never wait for one subagent to finish before starting another whose inputs are already known. Collect all subagent results first, then synthesize and make decisions.

**Delegate all exploration.** Use `Explore` subagents for any codebase search: finding where a function is tested, locating all call sites, checking whether a log field is used consistently across files. Do not run these searches in the main context.

**Delegate research.** When evaluating whether a comment, test, or log line is correct, delegate verification work to a subagent: read the referenced code, check whether a struct's `Debug` impl exposes sensitive fields, or verify a constant's derivation. The main agent never does these lookups in its own context.

**Subagents are read-only and flat.** A subagent must never write to, edit, or delete any file, and must never spawn child subagents. Its only job is to read, analyze, and report findings back to the main agent. Only the main agent spawns subagents and performs writes; writes and any next-batch dispatches happen only after every subagent in the current batch has returned (and then may run concurrently per the pipeline rule below). Within a batch, all subagents are dispatched in parallel — the per-batch synchronization point is when every subagent in it returns, not anywhere inside it.

**Edit in a dedicated phase.** A batch is all subagents spawned for a single source file. Once every subagent in the batch has returned, apply all edits to that file in one sequential pass. Never let a subagent read a file the main agent is currently editing — a subagent reading a file mid-edit will report stale findings.

**Pipeline reads with writes.** The edit phase must not become a bottleneck while subagents sit idle. As soon as a batch's subagents return, the main agent may dispatch the next batch concurrently with applying the current batch's edits — subject to a strict invariant: **no subagent in the new batch may read the file being edited, via any path.** Before every pipelined dispatch, verify all of the following:

- The next file's source does not import the editing file.
- The next file's test file does not import the editing file.
- The editing file does not import the next file (which would place the next file's call sites inside the editing file).
- The two files do not share a test file, fixture, or other read dependency.
- The new batch has no planned codebase-wide search (log-field consistency, global usage scans, etc.) that would traverse the editing file.

If you cannot prove every condition holds, defer the spawn until the edit phase completes. When in doubt, defer — a correct sequential run beats a fast run with stale findings. If all conditions hold, fire immediately so reads overlap writes.

**Flag cross-file dependencies.** If a subagent finds that a function's behavior is defined, constrained, or called from one or more other files not yet reviewed, do not act on the finding immediately. Note the dependency and resolve it once every involved file has been fully reviewed, so decisions are made against consistent state.

**Main agent responsibilities only:** synthesizing subagent findings into concrete edits, resolving ambiguity that requires cross-function or cross-file context, and making the final call on borderline cases. Everything that is pure information-gathering or parallel-safe work goes to a subagent.

---

# Review Pass

The per-file, per-function checklist lives in [TODO.md](TODO.md). Each item there is tagged with one of three review types (`Do comments`, `Do testing`, `Do logging`). Read this section before starting any pass so every decision is made against the same standard.

**Subagents report; the main agent acts.** The criteria below ("Remove it if", "Add one if", etc.) are evaluation standards, not edit commands. When a subagent works through a function it identifies which criteria are met and reports those findings. The main agent reads all findings and applies the edits to the source, then checks the corresponding box in [TODO.md](TODO.md). A subagent that makes a direct edit violates the read-only rule.

**Shared test files are edited last.** Integration test files shared across multiple source files must not be edited during any individual source file's edit phase. Queue all pending edits to shared test files and apply them in a single pass after every source file batch has completed, so no subagent ever reads a partially-edited shared test file.

---

## Do comments

**Goal:** every comment in the item earns its place by explaining something
the code cannot express on its own. No comment exists because it seemed like
a good idea at the time — each one is load-bearing.

Work through the item's source. For each comment you find — or for the
absence of one — ask:

**Remove it if:**

- It restates what the identifier or signature already says.
  (`// returns the length` above `fn len() -> usize` adds nothing.)
- It describes _what_ the code does rather than _why_ it does it that way.
- It's a leftover from a refactor, references a ticket, names a caller, or
  describes the "old" behavior.
- It's a section divider, spacer, or `// ===` banner with no information.

**Rephrase or correct it if:**

- It's accurate but wordy — cut to the minimum that preserves meaning.
- It uses vague language ("handle", "process", "deal with") where a precise
  verb exists.
- It refers to a type, function, or constant that has since been renamed.
- It describes a constraint or invariant imprecisely (e.g. says "non-negative"
  when the real bound is "strictly positive").
- The tone is a note-to-self or apology rather than a statement of fact.

**Add one if:**

- There is a non-obvious invariant the caller must respect (e.g. "must be
  called with the journal lock held", "input must be pre-normalized").
- The implementation chose a surprising approach for a non-obvious reason
  (e.g. a workaround for an Azalea bug, a deliberate performance trade-off).
- A value or formula is not derivable from the surrounding context (e.g. a
  magic timeout chosen to match a specific server tick rate).
- The function has a subtle failure mode or edge case that would bite a future
  reader who skims the signature.

**Keep it as-is only if** none of the above criteria apply.

The bar: after this pass, every comment left in the file should be one that a
future maintainer would be glad exists. If you're unsure whether to add one,
don't — silence is better than noise.

---

## Do testing

**Goal:** the test suite covers every meaningful behavior and failure mode,
names tests so their intent is clear without reading the body, and contains
no dead, redundant, or misleading tests.

Work through the item's source and its existing tests. For each behavior,
branch, and invariant, ask:

**Add a test if:**

- A non-trivial code path has no coverage (happy path, error path, edge case).
- An invariant is asserted in production code (`assert!`, bounds check,
  state-machine guard) but no test deliberately exercises the violation.
- A bug was fixed here and no regression test was added at the time.
- The function has a documented constraint (e.g. "input must be sorted") that
  is never verified by a test.
- Behavior depends on ordering, timing, or size thresholds — test at the
  boundary, not just safely inside it.

**Change a test if:**

- It passes for the wrong reason (tests the mock rather than the code,
  or asserts on an intermediate state that doesn't reflect the public contract).
- It is fragile — tightly coupled to implementation details that change
  without the behavior changing.
- The setup is so verbose it obscures what scenario is actually being tested —
  collapse repetitive construction into a builder or fixture helper.
- A `#[should_panic]` test doesn't assert the message, making it pass on any
  panic for any reason.

**Remove a test if:**

- It is dead — never runs because of a `#[cfg]` gate that is permanently
  unsatisfied, or because of wrong module placement.
- It tests a private helper that no longer exists or was inlined.
- It covers the same scenario as another test — same inputs and assertions,
  regardless of name.

**Re-evaluate (don't blindly remove) if:**

- It is marked `#[ignore]`. This is a deliberate skip, usually flagging a
  flaky test, an env-dependent test, or a TODO. Decide whether to resurrect
  it, rewrite it, or remove it with a commit-message note on why.

**Rename a test if:**

- The name describes the mechanism rather than the behavior
  (`test_parse_ok` → `buy_command_accepts_lowercase_item_name`).
- The name is generic (`test1`, `test_basic`, `test_edge_case`).
- The name says what the function does but not what the test verifies.

**Structure to aim for:** each test should read as a three-part sentence —
given (setup), when (action), then (assertion) — even if it's not literally
split that way. A reader should understand what scenario is being tested and
what correct behavior looks like without reading the production code.

---

## Do logging

**Goal:** the log output tells an operator exactly what the system is doing
and what went wrong, with no noise that drowns the signal and no silence that
hides a problem.

Work through the item's source. For each `tracing` call — or for its absence
— ask:

**Remove it if:**

- It is `trace!` and not genuinely needed for a specific deep-debugging
  session — `trace!` should not survive in production code by default.
- It fires on every iteration of a hot loop and produces volume with no
  diagnostic value.
- It duplicates information already present in a nearby call at the same or
  higher level.
- It logs internal implementation steps that an operator cannot act on and a
  developer can get from a debugger.
- The message is always the same string with no variables — it tells you
  _that_ something happened but not _what_ or _which_.

**Change the level if:**

- `error!` is used for a recoverable condition the system handles itself
  → downgrade to `warn!`.
- `warn!` is used for a condition that always requires operator intervention
  → upgrade to `error!`.
- `info!` is used for per-item or per-request events in a tight loop
  → downgrade to `debug!`.
- `debug!` is used for the top-level lifecycle events an operator would want
  to see in a tailed log → upgrade to `info!`.

**Change the message or fields if:**

- The message uses vague language ("failed", "error", "problem") without
  naming what failed and why.
- Key identifiers (item name, user UUID, chest ID, order ID) are missing
  from a message that fires in a context where multiple instances are
  possible — the operator cannot tell _which_ one without them.
- The message is a sentence fragment rather than a complete, human-readable
  description of what happened.
- The error value is dumped raw (`format!("{:?}", e)`) instead of extracting
  the relevant detail — `{e}` or a targeted field is almost always cleaner.
- Units are missing from numeric values (log `"timeout after 45s"` not
  `"timeout after 45000"`).

**Add a call if:**

- A branch is taken that permanently changes system state (pair deleted,
  node added, config reloaded) and no log records it.
- An error is swallowed with `let _ = ...` or `.ok()` and the failure is
  non-trivial (i.e. not "this is expected to fail sometimes").
- A recovery path is entered (retry, rollback, fallback) — the operator
  needs to know the system deviated from the happy path.
- Startup loads or skips a resource (pair file, user file, storage node)
  and the count or skipped item is not recorded.

**Keep it as-is only if** the level, message, and fields are all correct and
sufficient. The test: could an operator, reading only the log line, understand
what happened and decide whether action is needed?
