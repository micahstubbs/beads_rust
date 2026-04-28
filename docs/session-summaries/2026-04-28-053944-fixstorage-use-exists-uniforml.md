# Session Summary

**Date:** 2026-04-28
**Focus:** Tracker recovery + two latent bugs surfaced and fixed

## Summary

Started with `/next` looking for work; what looked like 3 stale-open
issues turned into a real bug hunt that surfaced two distinct bugs in
the storage layer. Tracker is now fully empty (0 open issues, all
delivered work closed) and the lib test suite is fully green
(1502 pass, 0 fail). 5 commits + 1 auto-generated summary commit pushed.

## Completed Work

### Commits

- `4651396` fix(sync): route same-issue self-collisions through AUTOINCREMENT (beads_rust-q0c)
- `dd74090` chore(beads): close q0c (sync rebuild PK collision fix)
- `3e517b5` chore(beads): close 6s0p and 46y2 (delivered, stale-open)
- `a0b45bd` fix(storage): use EXISTS uniformly in label filter to dodge fsqlite IN+IN planner bug
- `9ff178f` docs: add session summary (auto-generated; this file)

### Bugs found and fixed

**1. `sync_comments_for_import` PK violation on bulk JSONL rebuild (q0c)**

`br doctor --repair` and `br sync --import-only --force` both crashed
with `PRIMARY KEY constraint failed` when rebuilding the DB from the
project's own JSONL. Root cause: the defensive collision check at
`src/storage/sqlite.rs:8255` only routed colliding inserts through
AUTOINCREMENT when the existing row was on a *different* issue. But
with a corrupt JSONL where two issues claim the same `comment.id`,
the first claimant's id gets AUTO-reallocated; if the chosen id then
matches a *later* comment in the same issue's comments array, the check
sees a colliding row owned by the same issue and falls through to the
WITH-id INSERT path — which crashes on the row that was just
AUTO-allocated.

Fix: drop the narrow `existing != issue_id` predicate. Route through
AUTOINCREMENT whenever any row occupies the requested id. Added two
regression tests covering the cross-issue and same-issue self-collision
cases.

**2. fsqlite query planner returns empty for `id IN (SELECT...)` + `IN (?)` (a0b45bd)**

Surfaced while investigating a pre-existing test failure
(`test_list_issues_combined_type_and_label_filters`) that had been
broken on `main`. Per-clause SQL bisect showed:

- `id IN (SELECT issue_id FROM labels WHERE label = ?)` alone — works
- `issue_type IN (?)` alone — works
- both ANDed together — returns `[]` (the bug)
- `EXISTS (SELECT 1 FROM labels...)` + `IN (?)` — works

This is an upstream fsqlite query planner bug. Worked around in
`append_label_membership_filters` by using EXISTS uniformly (matching
the shape that the existing multi-label path already used).

### Stale-open issues closed

- `beads_rust-yray` (P2 bug) — delivered in 67f6605
- `beads_rust-aawk` (P3 task) — delivered in dfdeacc + 36ed370
- `beads_rust-p7xo` (P4 debt) — delivered in 20eb0ef; prior close lost to JSONL drift
- `beads_rust-6s0p` (P4 debt) — delivered in ae6a1d9; prior close lost to JSONL drift
- `beads_rust-46y2` (P2 bug) — resolved by 3564583 + 38b9c4d + e9dd461 cascade

## Key Changes

### Files Modified

- `src/storage/sqlite.rs` — comment-import collision fix (q0c) + label filter EXISTS workaround + 2 regression tests
- `.beads/issues.jsonl` — 6 issue closes (q0c, yray, aawk, p7xo, 6s0p, 46y2)

### Verified

- 1502 lib tests pass, 0 fail (the previously-failing
  `test_list_issues_combined_type_and_label_filters` now passes)
- 4 named e2e_concurrency tests that 46y2 listed as failing all pass
  on HEAD release build
- Real JSONL (776 records) imports cleanly into a fresh DB via
  `br sync --import-only` after the q0c fix
- Fixed binary installed system-wide via `cargo install --path .`

## Pending / Next Session Context

### Two follow-ups identified, not done in-session:

1. **Release the q0c + label-filter fixes.** Cargo.toml is at 0.2.1
   locally; latest GitHub release is v0.1.45. Versions 0.2.0 and 0.2.1
   were both bumped locally (see `ee6ef48`, `ddfb615`) but never
   successfully released — 0.2.0's release CI failed on stale test
   snapshots, 0.2.1 was prepared but not tagged. Recommended next step:
   bump Cargo.toml to 0.2.2, run `git tag v0.2.2 && git push --tags` to
   trigger `.github/workflows/release.yml`. Workflow has reliability
   gates so a failed CI just leaves the tag without a published release.

2. **Upstream fsqlite issue for the IN+IN planner bug** filed at
   [Dicklesworthstone/frankensqlite#76](https://github.com/Dicklesworthstone/frankensqlite/issues/76).
   When the upstream fix lands and we bump fsqlite, the workaround in
   `append_label_membership_filters` can be reverted (or kept — EXISTS
   is just as correct).

### What the next session should know

- The `br` CLI display layer bug (issue-not-found for IDs that exist
  in JSONL) is now resolved by the q0c fix — the underlying cause was
  the rebuild path crashing during DB recovery, leaving the DB stuck
  with stale data from March 3.
- `br doctor` is a useful first step when CLI commands return empty
  unexpectedly — it surfaces the DB-vs-JSONL count drift.
- The system-wide `br` binary is now `0.2.1+a0b45bd` (installed
  via `cargo install --path .` in this session); it has the q0c +
  label-filter fixes baked in.
