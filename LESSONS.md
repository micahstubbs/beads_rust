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

## Meta-Lessons

- **Library OOM != System OOM**: Always check if the "out of memory" error is from an internal resource pool before assuming system memory exhaustion
- **Shared resource pools are dangerous**: When a pool serves both cache and active operations, the cache can starve the operations. The only safe fix is periodic pool release (connection reopen)
- **Debug tracing narrows scope fast**: Adding eprintln! to phase boundaries and loop iterations pinpointed the failure from "somewhere in import" to "Phase 3 at issue ~1479" in one run, saving hours of guesswork
- **Batch size bisection**: When a batch size works or fails deterministically, binary search for the threshold (500→100→50→25) is the fastest path to a working configuration
