# Mission

This is a systematic quality pass over the entire codebase, function by function. The goal is not to add things — it is to reach the right state: every comment that remains earns its place, every meaningful behavior has a test that would catch a regression, and every log line gives an operator exactly what they need to understand what the system is doing or what went wrong. Proceed file by file: batch-read all functions in a file in parallel, apply all three reviews, then move to the next. When this pass is complete, the codebase should be in a state where no comment is noise, no test is misleading, and no operator is flying blind.

## Agent strategy — three tiers

Work is distributed across three tiers. The main agent holds the global view, file coordinators own one source file each end-to-end, and read-only analyst subagents swarm the individual functions.

**Tier 1 — Main agent (you).** Plans batches, enforces the batch-composition rule, dispatches file coordinators in parallel, and checks TODO.md boxes when each file reports complete. The main agent does NOT read functions, write per-function edits, or run codebase searches itself. Everything else delegates down.

**Tier 2 — File coordinators (`general-purpose` subagent, one per file).** Owns a single source file end-to-end: spawns its own per-function analyst subagents in parallel, synthesizes their findings, applies the resulting edits directly to its file (Edit/Write), runs its own compile check (`cargo check` scoped to that file's warnings if possible), and reports back a concise summary. A coordinator never touches any file other than its assigned one, and never dispatches analysts that would read another coordinator's file in the same batch. The main agent's batch-composition rule (below) makes this safe.

**Tier 3 — Analyst subagents (`Explore`, spawned by a coordinator, one per function).** Read-only. Evaluates a single function against the three Review Pass checklists and returns findings with line numbers and concrete replacement text. Never edits. Never spawns further subagents. Reads only files outside the coordinator's batch (since intra-batch edits are being applied concurrently).

**Parallelism target.** Main fires N file coordinators concurrently in a batch. Each coordinator fires M analysts concurrently (one per function in its file). Aim for a batch that saturates the worker pool — e.g. 4–8 coordinators × 5–15 analysts each ≈ 40–80 concurrent workers. Size coordinators to files; the number of analysts inside each is whatever the function count dictates.

**Batch composition rule (carries across all tiers).** For any two files A and B assigned to coordinators in the same batch:

- A's source does not import B (and vice-versa).
- A's test file does not import B (and vice-versa).
- A and B share no test file, fixture, or other read dependency.
- No coordinator in the batch performs a codebase-wide search (log-field consistency, global usage scans, etc.) — those must be run as their own isolated batch with no concurrent edits.

If two files would violate this, put them in separate batches. The main agent verifies this before dispatching.

**Coordinator contract.** A coordinator's prompt must spell out:

1. Its assigned file (one absolute path).
2. The full list of functions it owns from TODO.md.
3. The review criteria source (link to Review Pass).
4. The invariant that it must not read or edit any file outside its assigned one (list the other concurrent-batch files explicitly so it knows what is off-limits).
5. That it must compile-check its file when done and report the result.
6. The return format: concise summary of changes applied + compile result + any flagged cross-file dependencies for the main agent to resolve later.

**Cross-file dependencies.** If an analyst surfaces a finding that requires knowledge of an unreviewed file, the coordinator notes it in its report rather than acting. The main agent collects these across batches and resolves once every involved file has been reviewed.

**Shared test files are edited last.** Integration test files shared across multiple source files must not be edited by any coordinator. Queue such edits and apply them in a final isolated batch after every source file has been reviewed, so no analyst ever reads a partially-edited shared test file.

**When in doubt, defer.** A correct sequential run beats a fast run with stale findings. If a coordinator cannot prove its batch is safe, the main agent narrows the batch.

---

# Review Pass

The per-file, per-function checklist lives in [TODO.md](TODO.md). Each item there is tagged with one of three review types (`Do comments`, `Do testing`, `Do logging`). Read this section before starting any pass so every decision is made against the same standard.

**Analysts report; coordinators act.** The criteria below ("Remove it if", "Add one if", etc.) are evaluation standards. Analyst subagents (tier 3) evaluate functions against them and return findings with line numbers and concrete replacement text. The file coordinator (tier 2) reads all findings, applies edits to its file, and compile-checks. The main agent (tier 1) receives the coordinator's summary and checks the box in [TODO.md](TODO.md). Analysts never edit; coordinators never touch files outside their assignment.

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
