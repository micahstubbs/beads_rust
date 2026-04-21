use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorkspaceHealth {
    Healthy,
    Degraded,
    Recoverable,
    Unsafe,
}

impl WorkspaceHealth {
    #[must_use]
    pub fn is_operable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }

    #[must_use]
    pub fn needs_recovery(self) -> bool {
        matches!(self, Self::Recoverable)
    }

    #[must_use]
    pub fn is_fatal(self) -> bool {
        matches!(self, Self::Unsafe)
    }
}

impl fmt::Display for WorkspaceHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Healthy => f.write_str("healthy"),
            Self::Degraded => f.write_str("degraded"),
            Self::Recoverable => f.write_str("recoverable"),
            Self::Unsafe => f.write_str("unsafe"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnomalyClass {
    DatabaseMissing,
    DatabaseNotSqlite,
    DatabaseCorrupt { detail: String },
    WalCorrupt,
    SidecarMismatch { has_wal: bool, has_shm: bool },
    TruncatedWal,
    DuplicateSchemaRows { name: String, count: i64 },
    DuplicateConfigKeys { key: String, count: i64 },
    DuplicateMetadataKeys { key: String, count: i64 },
    JsonlParseError { detail: String },
    JsonlConflictMarkers,
    DbJsonlCountMismatch { db_count: usize, jsonl_count: usize },
    JsonlNewer,
    DbNewer,
    StaleRecoveryArtifacts,
    BlockedCacheStale,
    NullInNotNullColumn { table: String, column: String },
    DirtyFlagMismatch { flag: String, expected: bool, actual: bool },
    ExportHashMismatch { db_hash: String, jsonl_hash: String },
    ChildCountDrift { issue_id: String, stored: i64, actual: i64 },
    JournalSidecarPresent,
    OrphanedLockFile,
}

impl AnomalyClass {
    #[must_use]
    pub fn severity(&self) -> WorkspaceHealth {
        match self {
            Self::DatabaseNotSqlite
            | Self::DatabaseCorrupt { .. }
            | Self::WalCorrupt
            | Self::DatabaseMissing
            | Self::DuplicateSchemaRows { .. }
            | Self::DuplicateConfigKeys { .. }
            | Self::DuplicateMetadataKeys { .. }
            | Self::TruncatedWal => WorkspaceHealth::Recoverable,

            Self::JsonlConflictMarkers | Self::JsonlParseError { .. } => WorkspaceHealth::Unsafe,

            Self::SidecarMismatch { .. }
            | Self::DbJsonlCountMismatch { .. }
            | Self::JsonlNewer
            | Self::DbNewer
            | Self::StaleRecoveryArtifacts
            | Self::BlockedCacheStale
            | Self::NullInNotNullColumn { .. }
            | Self::DirtyFlagMismatch { .. }
            | Self::ExportHashMismatch { .. }
            | Self::ChildCountDrift { .. }
            | Self::JournalSidecarPresent
            | Self::OrphanedLockFile => WorkspaceHealth::Degraded,
        }
    }
}

impl fmt::Display for AnomalyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabaseMissing => f.write_str("database file missing"),
            Self::DatabaseNotSqlite => f.write_str("database file is not SQLite"),
            Self::DatabaseCorrupt { detail } => write!(f, "database corrupt: {detail}"),
            Self::WalCorrupt => f.write_str("WAL file corrupt"),
            Self::SidecarMismatch { has_wal, has_shm } => {
                write!(f, "sidecar mismatch (WAL={has_wal}, SHM={has_shm})")
            }
            Self::TruncatedWal => f.write_str("truncated WAL sidecar (<32 bytes)"),
            Self::DuplicateSchemaRows { name, count } => {
                write!(
                    f,
                    "duplicate sqlite_master entries for '{name}' ({count} rows)"
                )
            }
            Self::DuplicateConfigKeys { key, count } => {
                write!(f, "duplicate config rows for key '{key}' ({count} rows)")
            }
            Self::DuplicateMetadataKeys { key, count } => {
                write!(f, "duplicate metadata rows for key '{key}' ({count} rows)")
            }
            Self::JsonlParseError { detail } => write!(f, "JSONL parse error: {detail}"),
            Self::JsonlConflictMarkers => f.write_str("JSONL contains merge conflict markers"),
            Self::DbJsonlCountMismatch {
                db_count,
                jsonl_count,
            } => {
                write!(
                    f,
                    "DB/JSONL count mismatch (db={db_count}, jsonl={jsonl_count})"
                )
            }
            Self::JsonlNewer => f.write_str("JSONL has newer data than database"),
            Self::DbNewer => f.write_str("database has newer data than JSONL"),
            Self::StaleRecoveryArtifacts => f.write_str("stale recovery artifacts present"),
            Self::BlockedCacheStale => f.write_str("blocked_issues_cache marked stale"),
            Self::NullInNotNullColumn { table, column } => {
                write!(f, "NULL in NOT NULL column {table}.{column}")
            }
            Self::DirtyFlagMismatch {
                flag,
                expected,
                actual,
            } => {
                write!(f, "dirty flag '{flag}' mismatch (expected={expected}, actual={actual})")
            }
            Self::ExportHashMismatch { db_hash, jsonl_hash } => {
                write!(
                    f,
                    "export hash mismatch (db={db_hash}, jsonl={jsonl_hash})"
                )
            }
            Self::ChildCountDrift {
                issue_id,
                stored,
                actual,
            } => {
                write!(
                    f,
                    "child_count drift for '{issue_id}' (stored={stored}, actual={actual})"
                )
            }
            Self::JournalSidecarPresent => f.write_str("journal sidecar present (incomplete transaction)"),
            Self::OrphanedLockFile => f.write_str("orphaned lock file (.beads.lock) present"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceClassification {
    pub health: WorkspaceHealth,
    pub anomalies: Vec<AnomalyClass>,
}

impl WorkspaceClassification {
    #[must_use]
    pub fn healthy() -> Self {
        Self {
            health: WorkspaceHealth::Healthy,
            anomalies: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_anomalies(anomalies: Vec<AnomalyClass>) -> Self {
        let health = anomalies
            .iter()
            .map(AnomalyClass::severity)
            .max()
            .unwrap_or(WorkspaceHealth::Healthy);
        Self { health, anomalies }
    }

    #[must_use]
    pub fn is_operable(&self) -> bool {
        self.health.is_operable()
    }

    #[must_use]
    pub fn needs_recovery(&self) -> bool {
        self.health.needs_recovery()
    }

    #[must_use]
    pub fn recovery_possible(&self) -> bool {
        !matches!(self.health, WorkspaceHealth::Unsafe)
    }
}

impl fmt::Display for WorkspaceClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.health)?;
        if !self.anomalies.is_empty() {
            write!(f, " ({} anomalies)", self.anomalies.len())?;
        }
        Ok(())
    }
}

#[must_use]
pub fn classify_file_state(db_path: &Path, jsonl_path: &Path) -> Vec<AnomalyClass> {
    let mut anomalies = Vec::new();

    if !db_path.is_file() && jsonl_path.is_file() {
        anomalies.push(AnomalyClass::DatabaseMissing);
    }

    if db_path.is_file()
        && let Ok(bytes) = std::fs::read(db_path)
        && (bytes.len() < 16 || !bytes.starts_with(b"SQLite format 3\0"))
    {
        anomalies.push(AnomalyClass::DatabaseNotSqlite);
    }

    let wal_path = db_path.with_extension("db-wal");
    let shm_path = db_path.with_extension("db-shm");
    let has_wal = wal_path.is_file();
    let has_shm = shm_path.is_file();

    if has_wal && !has_shm {
        anomalies.push(AnomalyClass::SidecarMismatch { has_wal, has_shm });
    }

    if has_wal
        && let Ok(meta) = std::fs::metadata(&wal_path)
        && meta.len() < 32
    {
        anomalies.push(AnomalyClass::TruncatedWal);
    }

    if jsonl_path.is_file()
        && let Ok(content) = std::fs::read_to_string(jsonl_path)
    {
        let has_conflict_markers = content.lines().any(|line| {
            line.starts_with("<<<<<<<")
                || line.starts_with(">>>>>>>")
                || line.starts_with("=======")
        });
        if has_conflict_markers {
            anomalies.push(AnomalyClass::JsonlConflictMarkers);
        }
    }

    let journal_path = db_path.with_extension("db-journal");
    if journal_path.is_file() {
        anomalies.push(AnomalyClass::JournalSidecarPresent);
    }

    let lock_path = db_path
        .parent()
        .map(|p| p.join(".beads.lock"))
        .unwrap_or_else(|| db_path.with_file_name(".beads.lock"));
    if lock_path.is_file() {
        anomalies.push(AnomalyClass::OrphanedLockFile);
    }

    anomalies
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_workspace() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("beads.db");
        let jsonl_path = dir.path().join("issues.jsonl");
        (dir, db_path, jsonl_path)
    }

    #[test]
    fn healthy_workspace_has_no_anomalies() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        let mut f = std::fs::File::create(&db_path).unwrap();
        f.write_all(b"SQLite format 3\0").unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        std::fs::write(&jsonl_path, "{\"id\":\"test-1\"}\n").unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert!(anomalies.is_empty());
        let classification = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(classification.health, WorkspaceHealth::Healthy);
        assert!(classification.is_operable());
    }

    #[test]
    fn missing_db_with_jsonl_is_recoverable() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        std::fs::write(&jsonl_path, "{\"id\":\"test-1\"}\n").unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert_eq!(anomalies.len(), 1);
        assert!(matches!(anomalies[0], AnomalyClass::DatabaseMissing));
        let classification = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(classification.health, WorkspaceHealth::Recoverable);
        assert!(classification.recovery_possible());
    }

    #[test]
    fn non_sqlite_db_is_recoverable() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        std::fs::write(&db_path, "this is not a sqlite file").unwrap();
        std::fs::write(&jsonl_path, "{\"id\":\"test-1\"}\n").unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert!(
            anomalies
                .iter()
                .any(|a| matches!(a, AnomalyClass::DatabaseNotSqlite))
        );
        let classification = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(classification.health, WorkspaceHealth::Recoverable);
    }

    #[test]
    fn conflict_markers_in_jsonl_is_unsafe() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        let mut f = std::fs::File::create(&db_path).unwrap();
        f.write_all(b"SQLite format 3\0").unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        std::fs::write(
            &jsonl_path,
            "<<<<<<< HEAD\n{\"id\":\"a\"}\n=======\n{\"id\":\"b\"}\n>>>>>>> branch\n",
        )
        .unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert!(
            anomalies
                .iter()
                .any(|a| matches!(a, AnomalyClass::JsonlConflictMarkers))
        );
        let classification = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(classification.health, WorkspaceHealth::Unsafe);
        assert!(!classification.recovery_possible());
    }

    #[test]
    fn sidecar_mismatch_is_degraded() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        let mut f = std::fs::File::create(&db_path).unwrap();
        f.write_all(b"SQLite format 3\0").unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        std::fs::write(&jsonl_path, "{\"id\":\"test-1\"}\n").unwrap();
        let wal_path = db_path.with_extension("db-wal");
        std::fs::write(&wal_path, [0u8; 64]).unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert!(
            anomalies
                .iter()
                .any(|a| matches!(a, AnomalyClass::SidecarMismatch { .. }))
        );
        let classification = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(classification.health, WorkspaceHealth::Degraded);
        assert!(classification.is_operable());
    }

    #[test]
    fn classification_uses_worst_anomaly() {
        let anomalies = vec![
            AnomalyClass::SidecarMismatch {
                has_wal: true,
                has_shm: false,
            },
            AnomalyClass::JsonlConflictMarkers,
        ];
        let classification = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(classification.health, WorkspaceHealth::Unsafe);
    }

    #[test]
    fn anomaly_severity_ordering_is_correct() {
        assert!(WorkspaceHealth::Healthy < WorkspaceHealth::Degraded);
        assert!(WorkspaceHealth::Degraded < WorkspaceHealth::Recoverable);
        assert!(WorkspaceHealth::Recoverable < WorkspaceHealth::Unsafe);
    }

    #[test]
    fn journal_sidecar_detected() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        let mut f = std::fs::File::create(&db_path).unwrap();
        f.write_all(b"SQLite format 3\0").unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        std::fs::write(&jsonl_path, "{\"id\":\"test-1\"}\n").unwrap();
        let journal_path = db_path.with_extension("db-journal");
        std::fs::write(&journal_path, b"journal data").unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert!(anomalies.iter().any(|a| matches!(a, AnomalyClass::JournalSidecarPresent)));
        let c = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(c.health, WorkspaceHealth::Degraded);
    }

    #[test]
    fn orphaned_lock_file_detected() {
        let (_dir, db_path, jsonl_path) = setup_workspace();
        let mut f = std::fs::File::create(&db_path).unwrap();
        f.write_all(b"SQLite format 3\0").unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        std::fs::write(&jsonl_path, "{\"id\":\"test-1\"}\n").unwrap();
        let lock_path = db_path.parent().unwrap().join(".beads.lock");
        std::fs::write(&lock_path, "pid:12345").unwrap();

        let anomalies = classify_file_state(&db_path, &jsonl_path);
        assert!(anomalies.iter().any(|a| matches!(a, AnomalyClass::OrphanedLockFile)));
        let c = WorkspaceClassification::from_anomalies(anomalies);
        assert_eq!(c.health, WorkspaceHealth::Degraded);
    }

    #[test]
    fn new_anomaly_classes_have_correct_severity() {
        assert_eq!(
            AnomalyClass::DirtyFlagMismatch {
                flag: "needs_flush".to_string(),
                expected: true,
                actual: false,
            }.severity(),
            WorkspaceHealth::Degraded
        );
        assert_eq!(
            AnomalyClass::ExportHashMismatch {
                db_hash: "abc".to_string(),
                jsonl_hash: "def".to_string(),
            }.severity(),
            WorkspaceHealth::Degraded
        );
        assert_eq!(
            AnomalyClass::ChildCountDrift {
                issue_id: "x-1".to_string(),
                stored: 3,
                actual: 2,
            }.severity(),
            WorkspaceHealth::Degraded
        );
        assert_eq!(AnomalyClass::JournalSidecarPresent.severity(), WorkspaceHealth::Degraded);
        assert_eq!(AnomalyClass::OrphanedLockFile.severity(), WorkspaceHealth::Degraded);
    }
}
