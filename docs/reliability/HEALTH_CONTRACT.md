# Workspace Health Contract

Canonical reference for beads_rust workspace health classification.
Executable implementation: `src/health.rs`.

## Health Levels

| Level | Meaning | Operations Allowed |
|-------|---------|-------------------|
| **Healthy** | All invariants hold | All |
| **Degraded** | Derived state stale or minor drift | All (with advisory) |
| **Recoverable** | Primary data intact, DB corrupted | Read-only until recovery |
| **Unsafe** | Interchange data corrupted beyond repair | None until manual fix |

## Failure Taxonomy

### Primary Data (SQLite)

| Anomaly | Severity | Detection | Recovery |
|---------|----------|-----------|----------|
| `DatabaseMissing` | Recoverable | File stat | Rebuild from JSONL |
| `DatabaseNotSqlite` | Recoverable | Header check (first 16 bytes) | Rebuild from JSONL |
| `DatabaseCorrupt` | Recoverable | fsqlite open / integrity_check | Rebuild from JSONL |
| `DuplicateSchemaRows` | Recoverable | sqlite_master GROUP BY HAVING | Rebuild from JSONL |
| `DuplicateConfigKeys` | Recoverable | Config table duplicate probe | DELETE+INSERT dedup |
| `DuplicateMetadataKeys` | Recoverable | Metadata table duplicate probe | DELETE+INSERT dedup |
| `NullInNotNullColumn` | Degraded | Schema-aware NULL scan | Backfill or rebuild |
| `WriteProbeFailed` | Recoverable | Rollback-only doctor write probe | Rebuild from JSONL before writes continue |

### Interchange Data (JSONL)

| Anomaly | Severity | Detection | Recovery |
|---------|----------|-----------|----------|
| `JsonlParseError` | Unsafe | Line-by-line parse attempt | Manual edit required |
| `JsonlConflictMarkers` | Unsafe | Scan for `<<<<<<<`/`=======`/`>>>>>>>` | Manual merge resolution |
| `DbJsonlCountMismatch` | Degraded | Compare issue counts | Re-export from DB |
| `JsonlNewer` | Degraded | Timestamp/hash comparison | Re-import to DB |
| `DbNewer` | Degraded | Timestamp/hash comparison | Re-export to JSONL |
| `ExportHashMismatch` | Degraded | Compare stored hash vs computed | Re-export |

### Sidecars (WAL/SHM/Journal)

| Anomaly | Severity | Detection | Recovery |
|---------|----------|-----------|----------|
| `WalCorrupt` | Recoverable | WAL header validation | Delete WAL, rebuild |
| `SidecarMismatch` | Degraded | WAL exists without SHM or vice versa | Delete orphan |
| `TruncatedWal` | Recoverable | WAL file < 32 bytes | Delete truncated WAL |
| `JournalSidecarPresent` | Degraded | File existence check | Delete journal (incomplete txn) |
| `StaleRecoveryArtifacts` | Degraded | Recovery temp files present | Clean up |
| `OrphanedLockFile` | Degraded | `.beads.lock` file stat | Remove if no live process |

### Derived State

| Anomaly | Severity | Detection | Recovery |
|---------|----------|-----------|----------|
| `BlockedCacheStale` | Degraded | Metadata key check | Lazy rebuild on next read |
| `ChildCountDrift` | Degraded | Compare stored vs actual dep count | Recompute |
| `DirtyFlagMismatch` | Degraded | Compare flag vs actual dirty state | Reset flag |

## Invariant Matrix

Each row is a workspace component; columns indicate which subsystem owns and validates it.

| Component | Owner | Startup Check | Write-Path Check | Sync Check | Doctor Check |
|-----------|-------|---------------|------------------|------------|--------------|
| SQLite header | storage | `open()` | - | - | integrity_check |
| Schema version | storage | `apply_migrations()` | - | - | schema version match |
| Issue rows | storage | - | `update_issue` | export | count / sample |
| Issue writeability | storage | - | mutation transaction | - | rollback-only write probe |
| Config KV | storage | `get_config` | `set_config` | - | duplicate probe |
| Metadata KV | storage | `get_metadata` | `set_metadata` | - | duplicate probe |
| WAL sidecar | fsqlite | implicit | implicit | - | existence + size |
| SHM sidecar | fsqlite | implicit | implicit | - | existence |
| Journal sidecar | fsqlite | - | - | - | existence |
| JSONL file | sync | - | - | parse + export | conflict markers |
| Export hash | sync | - | - | compare | compare |
| Dirty flag (needs_flush) | sync | staleness probe | set on write | clear on flush | compare |
| Blocked cache | storage | lazy rebuild | refresh after mutation | - | stale marker |
| Child counters | storage | - | update on dep add/remove | - | count vs query |
| Dependencies table | storage | - | add/remove_dependency | - | FK integrity |

## Evidence Bundle (Incident Capture)

When a field failure occurs, the following artifacts should be collected for diagnosis:

1. **`beads.db`** - Full database file (or SHA-256 if too large)
2. **`beads.db-wal`** - WAL sidecar if present
3. **`beads.db-shm`** - SHM sidecar if present
4. **`issues.jsonl`** - Full JSONL interchange file
5. **`br doctor --json`** - Structured diagnostic output
6. **`br doctor --repair --dry-run --json`** - Projected repair actions
7. **Environment**: OS, fsqlite version, beads_rust version
8. **Timeline**: last successful operation, operation that failed, error message
9. **`.br_history/`** - Recent JSONL backups (last 3)
10. **`metadata` table dump** - All key-value pairs
11. **`sqlite_master` dump** - Schema state

### Capture Command

```sh
br doctor --bundle /tmp/incident-$(date +%Y%m%d-%H%M%S).tar.gz
```

(Not yet implemented - tracked for future work.)

## Severity Escalation Rules

- Multiple Degraded anomalies do NOT escalate to Recoverable
- Any single Recoverable anomaly blocks writes until resolved
- Any single Unsafe anomaly blocks ALL operations
- Composite health = max(individual severities)

## Contract Versioning

This contract is versioned alongside the `AnomalyClass` enum in `src/health.rs`.
Adding a new variant is backwards-compatible. Changing severity of an existing
variant requires a migration note in the changelog.
