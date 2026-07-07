#!/usr/bin/env python3
"""Resolve the 2026-07-06 upstream merge conflicts in src/storage/sqlite.rs.

Decisions (documented in docs/upstream-reviews/2026-07-06-dicklesworthstone-main.md):
  H1 label filter        -> UPSTREAM (their rewrite on fixed fsqlite supersedes our EXISTS dodge)
  H2 open_with_timeout   -> UPSTREAM structure + our WAL-aware connection_user_version
                            + retained DDL-canonicalization fast-path branch
  H3 open_current_read_only -> our WAL-aware version gate + upstream's struct fields
  H4 helper functions    -> UNION (keep connection_user_version and remove_temp_db_files)
  H5 comment import      -> UPSTREAM call; their new collision handler re-fixed separately
  H6 regression tests    -> UNION (keep our q0c self-collision test and upstream tests)
"""
import re
from pathlib import Path

p = Path("src/storage/sqlite.rs")
s = p.read_text()

hunk_re = re.compile(r"<<<<<<< HEAD\n(.*?)=======\n(.*?)>>>>>>> upstream/main\n", re.S)
hunks = hunk_re.findall(s)
assert len(hunks) == 6, f"expected 6 hunks, found {len(hunks)}"

resolutions = []

# H1: take upstream
resolutions.append(hunks[0][1])

# H2: upstream structure, connection-based version read, keep canonicalization branch
h2 = hunks[1][1]
old = """        let schema_current = database_header_user_version(path)
            .is_some_and(|version| version >= u32::try_from(CURRENT_SCHEMA_VERSION).unwrap_or(0));
        let runtime_compatible = runtime_schema_compatible(&conn);

        if schema_current && runtime_compatible {
            crate::storage::schema::apply_runtime_pragmas(&conn)?;
        } else if runtime_compatible {
"""
new = """        // Read the version through the connection, not the raw file header:
        // header bytes miss WAL-resident values, and a stale read here caused
        // spurious schema re-application against live databases under
        // concurrent sessions (treasury-jqms).
        let schema_current = connection_user_version(&conn)
            .is_some_and(|version| version >= u32::try_from(CURRENT_SCHEMA_VERSION).unwrap_or(0));
        let runtime_compatible = runtime_schema_compatible(&conn);

        if schema_current && runtime_compatible {
            if issues_table_create_sql_preserves_if_not_exists(&conn) {
                // bd-created DBs store `CREATE TABLE IF NOT EXISTS issues` DDL,
                // which breaks bd's ALTER TABLE reparse; canonicalize via the
                // compatible-schema path before taking the fast path.
                apply_runtime_compatible_schema(&conn)?;
            } else {
                crate::storage::schema::apply_runtime_pragmas(&conn)?;
            }
        } else if runtime_compatible {
"""
assert old in h2, "H2 upstream shape changed"
resolutions.append(h2.replace(old, new))

# H3: WAL-aware gate + upstream struct fields
resolutions.append(
    """        if connection_user_version(&conn)
            .is_none_or(|version| version < current_schema_version)
        {
            return Ok(None);
        }
        Ok(Some(Self {
            conn,
            mutation_count: 0,
            temp_db_path: None,
            pending_event_attribution: None,
        }))
"""
)

# H4: union — our helper first, then upstream's
resolutions.append(hunks[3][0] + hunks[3][1])

# H5: take upstream one-liner
resolutions.append(hunks[4][1])

# H6: union — our regression test plus upstream's content
resolutions.append(hunks[5][0] + hunks[5][1])

it = iter(resolutions)
s = hunk_re.sub(lambda m: next(it), s)
assert "<<<<<<<" not in s

# Re-fix upstream's self-collision guard in insert_comment_for_import (q0c):
old_guard = """                match self.import_comment_id_owner(comment.id)? {
                    Some(owner_issue_id) if owner_issue_id != issue_id => {
                        self.insert_import_comment_without_id(issue_id, comment, &created_at)
                    }
                    _ => Err(BeadsError::Database(error)),
                }"""
new_guard = """                match self.import_comment_id_owner(comment.id)? {
                    // Route through AUTOINCREMENT whenever ANY row owns the id —
                    // including this issue itself: an earlier comment in this same
                    // import call may have had its id AUTO-reallocated into the
                    // range of a later comment's JSONL id (beads_rust-q0c
                    // self-collision; see LESSONS.md 2026-04-28).
                    Some(_) => {
                        self.insert_import_comment_without_id(issue_id, comment, &created_at)
                    }
                    None => Err(BeadsError::Database(error)),
                }"""
assert old_guard in s, "upstream insert_comment_for_import guard shape changed"
s = s.replace(old_guard, new_guard)

p.write_text(s)
print("resolved: 6 hunks + q0c guard re-fix")
