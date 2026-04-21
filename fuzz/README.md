# Fuzz Targets

This directory contains cargo-fuzz targets for `beads_rust`.

Run the JSONL import harness in bounded mode:

```bash
cargo fuzz run jsonl_import -- -runs=10000 -max_len=65536
```

Run a compile check without fuzzing:

```bash
cargo fuzz check jsonl_import
```

The harnesses create isolated temporary workspaces and must not read or write
the repository's real `.beads/` directory.
