# Lessons Learned

## 2026-03-03T09:48 - fsqlite Shared PageBufPool Causes Hard OOM on Bulk Inserts

**Problem**: `br sync --rebuild --force` on a project with 2,257 issues (2.3MB JSONL) caused "Database error: out of memory" and WAL corruption despite the system having 62GB RAM and 80GB swap. The OOM was not a system-level OOM but an internal fsqlite error.

**Root Cause**: fsqlite (frankensqlite) has a `PageBufPool` with a hard limit of 1024 page buffers. This pool is **shared** between the ARC page cache and transaction write-sets (`pager.rs:354: let pool = inner.cache.pool().clone()`). As the B-tree grows during bulk inserts, dirty pages accumulate in the write-set. The `write_page` codepath (`pager.rs:983`) does `self.pool.acquire()?` with NO fallback on OOM — unlike `fetch_page` which can fall back to reading directly from disk. Once the combined cache + write-set exceeds 1024 slots, every subsequent write fails with `FrankenError::OutOfMemory`.

**Lesson**: When a library reports "out of memory" but the system has plenty of RAM, the OOM is likely an internal resource pool exhaustion, not a system memory issue. Investigate the library's internal memory management before trying system-level fixes. Shared resource pools between cache and write paths are a common source of this — the cache grows to fill the pool, leaving no room for writes.

**Code Issue**:
```rust
// Before (broken): Single giant transaction for all inserts
storage.begin_import_batch()?;
for (issue, action) in &import_ops {
    process_import_action(storage, action, issue, &mut result)?;
    // B-tree pages accumulate in write-set, eventually exhausting pool
}
storage.commit_import_batch(false)?;

// After (fixed): Small batches with connection reopen
const IMPORT_BATCH_SIZE: usize = 25;
storage.begin_import_batch()?;
let mut batch_count = 0;
for (issue, action) in &import_ops {
    process_import_action(storage, action, issue, &mut result)?;
    batch_count += 1;
    if batch_count >= IMPORT_BATCH_SIZE {
        storage.commit_import_batch(true)?;  // true = reopen connection
        storage.begin_import_batch()?;
        batch_count = 0;
    }
}
storage.commit_import_batch(true)?;
```

**Solution**: Three-part fix:
1. **Empty-DB fast path** — skip 3 collision-detection SELECTs per issue when rebuilding into an empty DB
2. **Batch inserts (25)** — commit every 25 inserts with WAL checkpointing to limit write-set size
3. **Connection reopen** — drop and reopen the fsqlite Connection between batches to fully release the shared PageBufPool (the only way to reclaim pool slots held by the ARC cache)

**Prevention**:
- When using fsqlite for bulk operations, always batch writes with connection reopen
- `PRAGMA cache_size` and `PRAGMA shrink_memory` do NOT help — they control different layers than the PageBufPool
- The failure point is deterministic (same issue count regardless of cache settings), which is a strong signal of a fixed internal limit rather than a system resource issue
- Add debug tracing (eprintln!) early in investigation to narrow the failure to a specific phase/loop iteration before diving into library source code

## 2026-04-28T03:00 - Rebase Conflicts Where HEAD Already Has the Fix Are "Obsolete Commits"

**Problem**: A 18-commit rebase onto an updated base produced conflicts on 6 commits — each one a small bug fix (cycle-detection filter, transaction wrapping, BEGIN IMMEDIATE, HashSet visited-set, batch import OOM). For each conflict, the incoming change was a localized patch, while HEAD had a much broader refactor that already contained the fix in a more thorough form (e.g., `with_write_transaction` helper replacing 5 manually-written BEGIN IMMEDIATE/COMMIT/ROLLBACK blocks; streaming `stream_import_actions_in_tx` replacing batch-and-reopen).

**Root Cause**: When two branches independently fix the same bug — one with a minimal patch, the other inside a broader refactor — `git` cannot merge them because they touch the same lines with structurally different code. But the fix's *intent* exists on both sides.

**Lesson**: After resolving a conflict by taking HEAD's version, run `git diff --cached HEAD` on the staged result. If the diff is empty, the commit is fully obsolete — its goal is already satisfied — and `git rebase --skip` is the correct resolution. This is much safer than `git rebase --continue` with a hand-edited commit, because skip preserves history integrity (no empty commits, no misleading "applied" status).

**Pattern**:
```bash
# After resolving a conflict block:
git checkout --ours -- <file>     # take HEAD's version
git add <file>
git diff --cached HEAD --stat     # measure residual delta

# If empty → commit is obsolete:
git rebase --skip

# If non-empty but expected → keep going:
git rebase --continue
```

**Solution**: Worked through each conflict by:
1. Reading both sides of the conflict + git show of the incoming commit
2. Confirming HEAD's broader refactor already contained the fix's intent
3. Taking HEAD wholesale (`git checkout --ours -- <file>`)
4. Verifying with empty `git diff --cached HEAD`
5. `git rebase --skip` — six times, one for each obsolete commit

**Prevention**:
- Before resolving conflicts manually line-by-line, check if HEAD's broader version already addresses the incoming commit's intent (read its commit message, then grep HEAD for the same fix)
- When the answer is "yes, already fixed," `--ours` + `--skip` is faster, safer, and more accurate than hand-merging
- The smoke test for "obsolete commit" is the empty `git diff --cached HEAD` after taking HEAD — no need to read the full file diff

## 2026-04-28T03:30 - SQL `INSERT...SELECT` Does Not Apply DEFAULT for Legacy NULLs

**Problem**: After upgrading br 0.1.44 → 0.2.1, every command (`list`, `ready`, `doctor`, …) failed against the project's own DB with `NOT NULL constraint failed: issues_rebuild_tmp.design`. Only `bv` (the JSONL sidecar reader) survived. `apply_schema` calls `rebuild_issues_table` whenever column order is non-canonical — this fires for any user upgrading from any older schema.

**Root Cause**: `rebuild_issues_table_inner` did `INSERT INTO issues_rebuild_tmp (cols) SELECT cols FROM issues` to copy data into the new strict schema. The new temp table declared `description`/`design`/`acceptance_criteria`/`notes`/`source_repo` as `TEXT NOT NULL DEFAULT ''`. Legacy rows held NULL in those columns (introduced when an older br version added them via ALTER TABLE without a backfill). The DEFAULT clause **does NOT** substitute for explicit `INSERT...SELECT` of a NULL value — DEFAULT only applies to (a) `INSERT` statements that omit the column entirely, or (b) `ALTER TABLE ADD COLUMN` backfilling existing rows. So the SELECT pushed the NULL through and it tripped the `NOT NULL` constraint, aborting the whole rebuild and leaving br broken.

**Lesson**: Whenever you tighten a SQL schema (nullable → `NOT NULL DEFAULT 'x'`), the rebuild/migration path **must** explicitly COALESCE the SELECT side. The CREATE TABLE side's DEFAULT clause is a phantom safety net here — it doesn't fire for `INSERT...SELECT`.

**Code Issue**:
```rust
// Before (broken)
let copy_out_sql = format!(
    "INSERT INTO issues_rebuild_tmp ({cols}) SELECT {cols} FROM issues",
    cols = projected_columns.join(", ")
);

// After (fixed)
// For each NOT NULL DEFAULT <literal> column, wrap in COALESCE(col, <literal>):
let copy_out_sql = format!(
    "INSERT INTO issues_rebuild_tmp ({cols}) SELECT {exprs} FROM issues",
    cols = projected_columns.join(", "),
    exprs = projected_select_exprs.join(", ")  // e.g. COALESCE(design, '')
);
```

**Solution**: Added `rebuild_select_expr(col_name, col_def)` + `parse_not_null_default_literal(col_def)` helpers in `src/storage/schema.rs`. The parser recognises `TEXT NOT NULL DEFAULT 'X'` (string literals: `''`, `'open'`, `'.'`) and `INTEGER NOT NULL DEFAULT N` (numeric literals: `0`, `2`). Non-literal defaults (`CURRENT_TIMESTAMP`) and nullable columns pass through unchanged — legacy NULLs in `created_at` would still fail loudly, which is correct (those represent corruption, not legacy data).

**Prevention**:
- Whenever `ISSUE_COLUMNS` (or any equivalent strict-schema declaration) gains a `NOT NULL DEFAULT` constraint that didn't exist before, audit every `INSERT...SELECT` path that copies data INTO that schema. Each one needs COALESCE for the new constraints.
- Write a regression test the moment you add a `NOT NULL DEFAULT` to an existing column. Test the path: legacy schema (column nullable) → row with NULL → rebuild → success with default substituted.
- For schema rebuilds that must be tolerant of legacy data (the common case during upgrades), prefer "tolerant SELECT, strict CREATE" over "strict SELECT". The SELECT is the data-cleanup hatch.

## Meta-Lessons

- **Library OOM != System OOM**: Always check if the "out of memory" error is from an internal resource pool before assuming system memory exhaustion
- **Shared resource pools are dangerous**: When a pool serves both cache and active operations, the cache can starve the operations. The only safe fix is periodic pool release (connection reopen)
- **Debug tracing narrows scope fast**: Adding eprintln! to phase boundaries and loop iterations pinpointed the failure from "somewhere in import" to "Phase 3 at issue ~1479" in one run, saving hours of guesswork
- **Batch size bisection**: When a batch size works or fails deterministically, binary search for the threshold (500→100→50→25) is the fastest path to a working configuration
- **Empty diff = obsolete commit**: During rebase, after resolving conflicts in favor of HEAD, an empty `git diff --cached HEAD` is the unambiguous signal to `--skip` rather than `--continue`. Saves dozens of bad merges across long rebases.
- **DEFAULT is for omission, not NULL**: SQL DEFAULT clauses fire on column omission or ALTER TABLE ADD backfills. They do NOT fire when an explicit `INSERT...SELECT` pushes a literal NULL into a `NOT NULL DEFAULT 'x'` column. Schema-tightening migrations must COALESCE on the SELECT side.
- **Sidecar surviving = data path bug, not data loss**: When a read-only sidecar (bv) shows correct data but the writer (br) crashes on open, the bug is in the writer's startup/migration path. Don't panic-restore; the data is fine.

## 2026-04-28T11:10 - Same-Issue Self-Collision After AUTOINCREMENT Re-allocation

**Problem**: `br doctor --repair` and `br sync --import-only --force` both crashed with `internal error: VDBE halted with code 19: PRIMARY KEY constraint failed` while rebuilding the SQLite DB from a healthy 776-record JSONL. Validation passed (`OK jsonl.parse: Parsed 775 records`), the DB was freshly created, and the source JSONL had no top-level duplicate keys — yet the rebuild crashed every time.

**Root Cause**: `sync_comments_for_import` had a defensive check at `src/storage/sqlite.rs:8255` that routed colliding-id inserts through AUTOINCREMENT — but ONLY when the existing row was on a *different* issue (`existing_issue_id != issue_id`). With a corrupt JSONL where two issues claim the same `comment.id`, the first claimant gets its id AUTO-reallocated. The AUTO-allocated id is `max(seq, max(id)) + 1`, which can land in the range of *future* JSONL-provided ids. When a later issue's comments array contains that allocated id as its own JSONL id, the check sees a colliding row owned by the same issue, falls through to the WITH-id INSERT branch, and crashes on the row that was just AUTO-allocated.

**Lesson**: Defensive collision routing must consider self-induced collisions, not just collisions with pre-existing data. The instinct to write `existing_issue_id != issue_id` is "another issue owns it; sidestep" — but the same logic skipped over "I just took that slot myself with an auto-allocated id." When a function loops over inputs and may mutate state that future iterations read, treat the function's own intermediate writes as part of the conflict surface.

**Code Issue**:
```rust
// Before (broken): "different issue" check is too narrow
if colliding_issue_id
    .as_deref()
    .is_some_and(|existing_issue_id| existing_issue_id != issue_id)
    || comment.id <= 0
{
    // AUTOINCREMENT path
} else {
    // WITH-id INSERT — fails when "issue_id" is THIS call's own
    // earlier AUTO-allocated row that happens to occupy comment.id.
}

// After (fixed): if any row owns the slot, route through AUTO
if colliding_issue_id.is_some() || comment.id <= 0 {
    // AUTOINCREMENT path
}
```

**Solution**: Drop the narrow `existing != issue_id` predicate. The `DELETE FROM comments WHERE issue_id = ?` at the top of the function clears any prior rows for THIS issue from the *previous* call; any remaining colliding row inside the loop is either another issue's row or this issue's just-auto-allocated row — AUTO is the correct path in both cases. See commit 4651396.

**Prevention**:
- When a defensive check sidesteps "rows owned by someone else", explicitly think about self-induced collisions: rows this call has written that future iterations of the same call will encounter.
- A regression test that exercises self-collision is hard to build without deliberately pinning the AUTOINCREMENT counter (see `test_sync_comments_for_import_handles_same_issue_self_collision_after_auto_realloc` in src/storage/sqlite.rs). Pre-seed via `add_comment` to advance `sqlite_sequence`, then call the function with comment ids that intersect the auto-allocation range.

## 2026-04-28T12:00 - Library Bug That Only Fires With Bound Parameters, Not Inlined SQL

**Problem**: `list_issues(types=[Task], labels=[core])` returned `[]` against fsqlite, even though both filter clauses individually returned the matching row. The downstream test `test_list_issues_combined_type_and_label_filters` had been failing on `main` for some time; my first instinct was a beads_rust SQL-construction bug, but per-clause bisect showed the SQL was correct and the planner was returning empty. Filed it upstream as fsqlite#76 with an inlined-string repro, then tried to reproduce it in fsqlite's own test harness — and the inlined-string repro **passed**. The bug seemed to evaporate.

**Root Cause**: The bug only fires when both `IN (?)` literals are bound parameters via `query_with_params`. With inlined string literals (`label = 'core'` and `IN ('task')`), the planner chooses a different path that doesn't trigger the bug. beads_rust uses `query_with_params` exclusively for safety; fsqlite's own internal tests use inlined strings; that's why upstream's test suite never caught it.

**Lesson**: When a SQL bug "doesn't reproduce" against a library's own test harness, check whether the harness uses the *same calling shape* as the failing call. Inlined SQL and parameterized SQL go through different planner paths — same logical query, different IR. A test that exercises the bug shape via inlined strings is a *different test* from one that exercises it via bind parameters, even if the SQL text matches after substitution.

**Code Issue**:
```rust
// Inlined — works on fsqlite 0.1.2 (planner picks safe path)
conn.query("SELECT id FROM t WHERE id IN (SELECT issue_id FROM labels WHERE label = 'core') AND issue_type IN ('task')")

// Parameterized — returns [] on fsqlite 0.1.2 (planner bug)
conn.query_with_params(
    "SELECT id FROM t WHERE id IN (SELECT issue_id FROM labels WHERE label = ?) AND issue_type IN (?)",
    &[SqliteValue::from("core"), SqliteValue::from("task")],
)
```

**Solution**: Two parts:
1. Workaround in beads_rust: switch `append_label_membership_filters` to use `EXISTS (...)` uniformly, dodging the planner's troubled path entirely (commit a0b45bd).
2. Filed upstream — and discovered the bug is *already fixed on fsqlite main* (HEAD `e5c83f11`), just not yet released as 0.1.3. Opened upstream PR #77 with a regression test covering both inlined AND parameterized variants so the fix doesn't silently regress.

**Prevention**:
- When writing regression tests for SQL bugs, cover BOTH inlined and parameterized variants. They exercise different planner code.
- When investigating a downstream library bug, before diving into the library source: rebuild against the library's `main` branch (or git HEAD) to see if the bug is already fixed. fsqlite's published 0.1.2 was 5+ weeks behind main; my workaround was real but the upstream fix had already shipped.
- A library can have the same `version =` field on crates.io and on `main` while the code differs significantly. "Same version" is necessary but not sufficient evidence that you're testing the same code.

## More Meta-Lessons

- **Loops mutate the conflict surface they iterate over**: When a function loops over inputs and writes to a table that future iterations read, the function's own intermediate writes are part of the conflict surface. "Owner != me" checks must include "owner == me from earlier in this same call."
- **Inlined vs parameterized SQL = different code paths**: A SQL bug that doesn't reproduce against a library's tests may be hiding behind the calling shape. Inlined string literals and bind parameters go through different planner phases, even when the resulting query is logically identical. Always test both shapes for SQL bugs.
- **Check `main`, not just the published version**: Before fixing an upstream library bug, build against the upstream `main` branch — not just the latest published version. A library can have the same `version =` field on crates.io and in its repo while the code differs significantly. The fix you're about to write may already be one merged-but-unreleased commit away.
