# Upstream Review: Dicklesworthstone/beads_rust @ main

**Date:** 2026-07-06
**Status at review:** upstream 771 commits ahead (v0.2.17, fsqlite 0.1.13); fork carried 34 unique commits (v0.2.4, fsqlite 0.1.7)
**Strategy:** at 771:34 divergence, per-commit review inverts — merged `upstream/main` wholesale and treated our 34 commits as the patch set to preserve. Merge commit `7c2c4fc8`.

## Fork patch-set disposition

| Fork change | Decision | Reason |
|---|---|---|
| `connection_user_version` WAL-aware version reads (2405632, treasury-jqms) | **RETAINED, re-applied onto upstream structure** | Upstream still reads raw header bytes in both `open_with_timeout` and `open_current_read_only`; the concurrent-session corruption fix is not upstreamed. Upstream-worthy — consider a PR. |
| DDL canonicalization fast-path branch (df0f2a6) | **RETAINED** | bd-compat concern absent upstream; re-grafted into upstream's restructured `open_with_timeout`. |
| q0c self-collision AUTO routing (4651396) | **RETAINED as a re-fix of upstream's new code** | Upstream refactored comment import into `insert_comment_for_import` — and its collision handler has the *same* narrow `owner != issue_id` guard our LESSONS entry warned about. Dropped the guard so self-collisions route through AUTOINCREMENT; kept our regression test alongside upstream's. |
| EXISTS label-filter workaround (a0b45bd) | **SUPERSEDED — took upstream** | Their `IN … GROUP BY … HAVING COUNT(DISTINCT)` rewrite with dedup runs on fsqlite 0.1.13, where the IN+IN planner bug is fixed. |
| NULL-coalesce rebuild helpers (0b0b233b) | **KEPT ALONGSIDE upstream's** | Upstream added `backfill_storage_null_in_default_columns` (broader, typeof-based, their #269); ours feeds our retained rebuild path. Both callers live; both kept. |
| fsqlite 0.1.7 bump (72f07a1) | **SUPERSEDED** | Upstream pins 0.1.13. |
| Commit Cargo.lock (b9ec2522) | **CONVERGED** | Upstream independently started tracking Cargo.lock for the same binary-reproducibility reason; took their lockfile (matches their dep graph). |
| LESSONS.md, session summaries, skills, codebase-review docs, cross-compiling doc | **RETAINED** | Fork-local records; upstream has no LESSONS.md. |
| Version/release commits (0.2.2–0.2.4) | **SUPERSEDED** | Fork now rebased on upstream v0.2.17; next fork release continues from there. |

## Conflict resolutions (6 files)

- `src/storage/sqlite.rs` (6 hunks) — per table above; mechanical script kept at `scripts/resolve-upstream-merge-2026-07-06.py`
- `src/storage/schema.rs` (2 hunks) — union: both NULL-handling helpers, both test assertions
- `Cargo.toml`, `Cargo.lock` — upstream
- `.gitignore` — upstream (which now tracks Cargo.lock)
- `.beads/issues.jsonl` — union by id: upstream's 905 records + 11 fork-only issues

## Verification gate for this merge

Build `--locked` on pinned `nightly-2026-02-19`; full `cargo test`; the fork's WAL-visibility test and q0c self-collision test both present and passing; concurrency stress (point lookups under sync churn) clean before deployment.
