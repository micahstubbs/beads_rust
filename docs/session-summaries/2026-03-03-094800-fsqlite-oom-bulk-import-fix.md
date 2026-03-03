# Session Summary: fsqlite OOM on Bulk Import Fix

## Summary

Fixed critical OOM bug in `br sync --rebuild --force` where importing 2,257 issues from a 2.3MB JSONL file caused fsqlite's internal page buffer pool to exhaust, producing "Database error: out of memory" and WAL corruption.

## Completed Work

- **0020fb0** - fix: resolve fsqlite OOM on bulk import by batching with connection reopen (beads_rust-19qx)

## Key Changes

### Files Modified
- `src/storage/sqlite.rs` - Added `db_path` field, `reopen()` method, bulk import batch API (`begin_import_batch`, `commit_import_batch`, `rollback_import_batch`, `end_bulk_import`), and `set_export_hashes_raw()`
- `src/sync/mod.rs` - Added empty-DB fast path in Phase 1, batched Phase 3 with connection reopen (batch size 25)

### Root Cause

fsqlite (frankensqlite) has a shared `PageBufPool` with a hard limit of 1024 page buffers. This pool is shared between the ARC page cache and transaction write-sets. The `write_page` codepath (`pager.rs:983`) does `self.pool.acquire()?` with NO fallback on OOM (unlike `fetch_page` which can read directly from disk). As the B-tree grows during bulk inserts, the write-set needs more page buffers for B-tree splits, eventually exhausting the pool.

Key discoveries:
- `PRAGMA cache_size` has zero effect (controls ARC logical size, not PageBufPool)
- `PRAGMA shrink_memory` has zero effect (only coalesces versioned entries, doesn't evict pages)
- Only dropping the Connection fully releases the shared buffer pool

### Three-Part Fix

1. **Fast path for empty DB**: When `count_issues() == 0`, skip all collision detection queries (saves 3 SELECTs per issue)
2. **Batched transactions**: Group inserts in batches of 25 with WAL checkpointing
3. **Connection reopen**: Drop and reopen the fsqlite connection between batches to fully release the shared page buffer pool

### Batch Size Investigation

| Batch Size | Reopen | Result |
|-----------|--------|--------|
| 500 | No | OOM at issue 874 |
| 100 | No | OOM at issue 1479 |
| 100 | Yes | OOM at issue 1476 |
| 50 | Yes | OOM at issue 1949 |
| 25 | Yes | All 2,257 imported |

## Verification

- All 120 sync tests pass
- 783/787 lib tests pass (4 pre-existing failures)
- Clean casemirror rebuild: 2,257 issues in 16 seconds
- DB healthy: 10.6MB, WAL at 32 bytes

## Pending/Blocked

None - fix is complete and verified.

## Next Session Context

- The remaining items from the codebase review (BUG-1 cycle detection, BUG-2 TOCTOU race) are tracked as separate beads issues
- BUG-3 (set_config/set_metadata transactions) and BUG-4 (rebuild_blocked_cache BEGIN IMMEDIATE) were already fixed in prior sessions
- PERF-1 (HashSet for O(1) lookup) was already fixed in f9424a7
