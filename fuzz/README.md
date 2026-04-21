# Fuzz Targets

This directory contains cargo-fuzz targets for `beads_rust`.

Run the JSONL import harness in bounded mode:

```bash
cargo fuzz run jsonl_import -- -runs=10000 -max_len=65536
```

Run the content-hash harness in bounded mode:

```bash
cargo fuzz run content_hash -- -runs=10000 -max_len=4096
```

Run a compile check without fuzzing:

```bash
cargo fuzz check jsonl_import
cargo fuzz check content_hash
```

The harnesses create isolated temporary workspaces and must not read or write
the repository's real `.beads/` directory.

The `content_hash` target treats JSON serialization whitespace, JSON field
order, unknown JSON fields, empty optional fields, labels, relationships, and
metadata-only issue fields as formatting-insensitive. Whitespace inside included
issue text fields such as title, description, design, acceptance criteria, and
notes is intentionally treated as meaningful issue content.
