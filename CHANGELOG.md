# Changelog

This is a synthesized, agent-facing changelog for the full history of `br`.

Scope window: project inception on 2026-01-16 through `v0.1.30` on 2026-03-21.

This document was rebuilt from the full git history, the version-tag/release history, and the checked-in beads tracker. It summarizes 914 non-merge commits across 31 version cuts.

This document is intentionally organized by landed capabilities, not raw diff order. Each major section includes:

- Accurate version links for every tagged cut
- Live commit URLs for representative implementation work
- Linked beads/workstreams so an agent can jump from the summary to the planning history

Bead links below use GitHub code search scoped to the checked-in `.beads/issues.jsonl` history so they land on the actual planning record instead of generic repo matches.

## Unreleased

### Added
- Comprehensive CHANGELOG.md generated from full git history (replaces placeholder).
- `doctor` warns when root `.gitignore` hides `.beads/.gitignore`.
- Concurrent close/update/reopen blocked-cache integrity stress test (e2e).

### Changed
- Release automation now generates `RELEASE_NOTES.md` instead of pretending a temporary release-body file is the repo's canonical changelog.
- Centralized ID resolution into `resolve_issue_id(s)` helpers across CLI commands.
- Lazy config loading to reduce sync lock contention; checkpoint-on-close opt-out.
- Downgraded auto-import `SyncConflict` to warning for concurrent writes.
- Storage schema: removed redundant index, simplified event inserts, added dependency thread index.
- Switched schema temp tables from `:memory:` to temp files with debug logging.
- Atomic config writes using PID-scoped temp files.

### Fixed
- `INSERT OR REPLACE` for `blocked_issues_cache` to prevent UNIQUE constraint errors during concurrent cache rebuilds.
- `INSERT OR REPLACE` for `export_hashes` and `child_counters` (defense-in-depth against concurrent races on PRIMARY KEY columns).
- Graceful fallback for missing dependencies in graph rendering instead of crashing.
- Graceful fallback for all blocked-cache read operations.
- Single-row blocked-cache inserts with deferred invalidation and large-batch regression test.
- `LazyLock` regex in agents; defer-first blocked-cache invalidation.
- JSONL validation improvements for sync and show commands.

* * *

## Version Timeline

`Kind` distinguishes a published GitHub Release from a bare git tag used for fast stabilization cuts.

| Version | Kind | Date | Summary |
|---------|------|------|---------|
| [`v0.1.30`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.30) | Release | 2026-03-21 | Blocked-cache hardening, stats/reporting expansion, list/docs fixes, concurrency safety follow-through |
| [`v0.1.29`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.29) | Release | 2026-03-19 | `frankensqlite` `v0.1.1` upgrade for major write-performance gains |
| [`v0.1.28`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.28) | Release | 2026-03-14 | Workspace-failure corpus, database-family snapshots, quarantine model, MCP/install fixes |
| [`v0.1.27`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.27) | Release | 2026-03-13 | Reliability wave around workspace health, concurrency coverage, and failure modeling |
| [`v0.1.26`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.26) | Release | 2026-03-11 | Routing/TOON/quiet-mode completion wave |
| [`v0.1.25`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.25) | Release | 2026-03-11 | Routing, output, and CLI/storage refactors before the late-March hardening wave |
| [`v0.1.24`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.24) | Release | 2026-03-08 | WAL concurrency fixes for multi-agent safety |
| [`v0.1.23`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.23) | Release | 2026-03-07 | Release cut after sync/config/CLI stabilization |
| [`v0.1.22`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.22) | Release | 2026-03-07 | Release cut in the middle of sync and compatibility hardening |
| [`v0.1.21`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.21) | Release | 2026-03-04 | Release cut before the large reliability/concurrency push |
| [`v0.1.20`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.20) | Release | 2026-02-26 | `fsqlite` aggregate crash fix, macOS VFS compatibility, sync flake improvements |
| [`v0.1.19`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.19) | Release | 2026-02-23 | Release cut after agent/install/release automation fixes |
| [`v0.1.18`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.18) | Tag | 2026-02-23 | GITHUB_TOKEN, Apple Silicon naming, dry-run, and Linux-build fixes |
| [`v0.1.17`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.17) | Tag | 2026-02-23 | GITHUB_TOKEN, Apple Silicon naming, agent dry-run, CI fix |
| [`v0.1.16`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.16) | Tag | 2026-02-23 | GITHUB_TOKEN, Apple Silicon naming, agent dry-run, review fixes |
| [`v0.1.15`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.15) | Tag | 2026-02-23 | Release automation, auth token, and asset naming improvements |
| [`v0.1.14`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.14) | Release | 2026-02-15 | Release cut during the sync-safety/agent-ergonomics phase |
| [`v0.1.13`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.13) | Release | 2026-02-01 | Release cut after rich-output and compatibility work |
| [`v0.1.12`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.12) | Release | 2026-01-29 | Release cut after early output-mode and schema/storage parity work |
| [`v0.1.11`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.11) | Tag | 2026-01-28 | Release cut during the first post-launch stabilization cycle |
| [`v0.1.10`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.10) | Tag | 2026-01-28 | Nightly/clippy stabilization cut |
| [`v0.1.9`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.9) | Tag | 2026-01-23 | ID-prefix validation and ready-query fixes |
| [`v0.1.8`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.8) | Tag | 2026-01-22 | Clippy/nightly compatibility fix |
| [`v0.1.7`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.7) | Release | 2026-01-18 | First broadly usable post-launch cut |
| [`v0.1.6`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.6) | Tag | 2026-01-18 | `cargo fmt`/CI correction |
| [`v0.1.5`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.5) | Tag | 2026-01-18 | Conformance CI skip fix |
| [`v0.1.4`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.4) | Tag | 2026-01-18 | Conformance CI fix |
| [`v0.1.3`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.3) | Tag | 2026-01-18 | Benchmark test fix for environments without `bd` |
| [`v0.1.2`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.2) | Tag | 2026-01-18 | Early CI and dependency stabilization |
| [`v0.1.1`](https://github.com/Dicklesworthstone/beads_rust/tree/v0.1.1) | Tag | 2026-01-18 | Immediate follow-up stabilization after public launch |
| [`v0.1.0`](https://github.com/Dicklesworthstone/beads_rust/releases/tag/v0.1.0) | Draft release | 2026-01-18 | Initial public release |

* * *

## 1) The Classic `beads` Port Landed as a Real Rust CLI

`br` began as a deliberate freeze of the classic SQLite + JSONL architecture, not a fork chasing the newer Gastown direction. The initial development wave established the Rust crate, the core model/storage layers, the command scaffold, and the non-invasive philosophy that still defines the project.

### Delivered capability

- Rust crate + nightly toolchain setup for a classic `beads` port.
- Core model types, validation, ID generation, content hashing, and logging.
- SQLite primary storage with JSONL import/export as the collaboration surface.
- Broad command-surface parity for init/create/list/show/update/close/reopen/ready/blocked/search/dep/comments/config/doctor/sync/history and related workflow commands.
- Explicit documentation of classic-only scope and non-invasive rules.

### Closed workstreams

- [`beads_rust-8f8`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-8f8%22+path%3A.beads%2Fissues.jsonl&type=code) EPIC: Port beads (SQLite+JSONL) to Rust as `br`
- [`beads_rust-g3i`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-g3i%22+path%3A.beads%2Fissues.jsonl&type=code) Phase 1: Foundation
- [`beads_rust-0ol`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-0ol%22+path%3A.beads%2Fissues.jsonl&type=code) Phase 2: Core Commands
- [`beads_rust-1ce`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-1ce%22+path%3A.beads%2Fissues.jsonl&type=code) Phase 3: Relations & Search
- [`beads_rust-1md`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-1md%22+path%3A.beads%2Fissues.jsonl&type=code) Phase 4: Sync & Config

### Representative commits

- [`38cd152`](https://github.com/Dicklesworthstone/beads_rust/commit/38cd152) added `AGENTS.md` and the original porting plan.
- [`ec14cba`](https://github.com/Dicklesworthstone/beads_rust/commit/ec14cba) initialized the Rust project and codified legacy behavior expectations.
- [`562e021`](https://github.com/Dicklesworthstone/beads_rust/commit/562e021) implemented the core model types.
- [`16c98b8`](https://github.com/Dicklesworthstone/beads_rust/commit/16c98b8) added the classic CLI command scaffold.
- [`229ec5a`](https://github.com/Dicklesworthstone/beads_rust/commit/229ec5a) brought in doctor diagnostics and sync CLI modules.
- [`5444b9b`](https://github.com/Dicklesworthstone/beads_rust/commit/5444b9b) landed the search command early in the port.

* * *

## 2) Sync Safety Became a First-Class System, Not a Hopeful Convention

One of the most important architectural themes in `br` is that sync must never mutate the surrounding repository in surprising ways. The sync engine evolved from basic JSONL import/export into a guarded, threat-modeled subsystem with explicit external-JSONL gating, 3-way merge logic, local history backups, SyncConflict handling, and crash-friendly snapshot/quarantine infrastructure.

### Delivered capability

- Explicit path allowlisting and canonicalization for sync I/O.
- Strong “no git operations” guarantees in both code and regression coverage.
- External JSONL opt-in instead of silent broad filesystem writes.
- 3-way merge infrastructure and conflict detection rather than last-write-wins guesswork.
- Local `.br_history` backups and failure-aware export/import behavior.
- Database-family snapshotting, sidecar quarantine, and incident-oriented recovery surfaces.

### Closed workstreams

- [`beads_rust-0v1`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-0v1%22+path%3A.beads%2Fissues.jsonl&type=code) Sync safety hardening epic
- [`beads_rust-eclx`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-eclx%22+path%3A.beads%2Fissues.jsonl&type=code) Sync safety & JSONL integrity
- [`beads_rust-07b`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-07b%22+path%3A.beads%2Fissues.jsonl&type=code) 3-way merge algorithm implementation
- [`beads_rust-w5c`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-w5c%22+path%3A.beads%2Fissues.jsonl&type=code) Local history backups + history command
- [`beads_rust-7nw`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-7nw%22+path%3A.beads%2Fissues.jsonl&type=code) Auto-flush + dirty tracking + export hash maintenance

### Representative commits

- [`cc605b2`](https://github.com/Dicklesworthstone/beads_rust/commit/cc605b2) enforced the sync JSONL allowlist and added the external opt-in flag.
- [`90544e2`](https://github.com/Dicklesworthstone/beads_rust/commit/90544e2) added structured sync-safety logging.
- [`246475a`](https://github.com/Dicklesworthstone/beads_rust/commit/246475a) introduced the 3-way merge infrastructure.
- [`1017b00`](https://github.com/Dicklesworthstone/beads_rust/commit/1017b00) added `SyncConflict` instead of risking silent data loss.
- [`968d2e0`](https://github.com/Dicklesworthstone/beads_rust/commit/968d2e0) re-read JSONL before flush in `--no-db` mode to avoid clobbering concurrent writes.
- [`e430d4c`](https://github.com/Dicklesworthstone/beads_rust/commit/e430d4c) added database-family snapshots, quarantine support, and the external JSONL safety model.

* * *

## 3) Testing, Conformance, Benchmarks, and Release Automation Went Deep

`br` did not stop at “commands seem to work.” The project built out extensive unit, E2E, conformance, snapshot, and benchmark infrastructure, plus release automation, installers, and CI guardrails. This is one of the main reasons the repo became usable so quickly despite the pace of change.

### Delivered capability

- Structured E2E harnesses with logging and artifact capture.
- `bd` vs `br` conformance harnesses to keep classic behavior grounded.
- Snapshot/golden testing for output surfaces.
- Synthetic and real-dataset benchmark suites.
- CI workflows, release workflows, installer automation, signing/checksum flows, and distribution plumbing.

### Closed workstreams

- [`beads_rust-btm`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-btm%22+path%3A.beads%2Fissues.jsonl&type=code) Testing coverage epic
- [`beads_rust-2cwe`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-2cwe%22+path%3A.beads%2Fissues.jsonl&type=code) Testing infrastructure & CI pipeline
- [`beads_rust-6t53`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-6t53%22+path%3A.beads%2Fissues.jsonl&type=code) Comprehensive conformance + benchmark expansion
- [`beads_rust-ag35`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-ag35%22+path%3A.beads%2Fissues.jsonl&type=code) Exhaustive E2E/conformance/benchmark harness
- [`beads_rust-7nh`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-7nh%22+path%3A.beads%2Fissues.jsonl&type=code) Installation & distribution automation

### Representative commits

- [`2634839`](https://github.com/Dicklesworthstone/beads_rust/commit/2634839) added the `br`/`bd` conformance test harness.
- [`f236d6b`](https://github.com/Dicklesworthstone/beads_rust/commit/f236d6b) added comprehensive test-run logging infrastructure.
- [`8d7f1d3`](https://github.com/Dicklesworthstone/beads_rust/commit/8d7f1d3) expanded conformance coverage for quick capture.
- [`c244848`](https://github.com/Dicklesworthstone/beads_rust/commit/c244848) strengthened CI with audits, caching, and matrix improvements.
- [`d24c7f4`](https://github.com/Dicklesworthstone/beads_rust/commit/d24c7f4) upgraded the release workflow with preflight checks, musl builds, and verification.
- [`1910db4`](https://github.com/Dicklesworthstone/beads_rust/commit/1910db4) improved dataset-registry commit detection for benchmark realism.

* * *

## 4) Rich Output and Agent-First Output Modes Became a Core Product Surface

`br` grew beyond plain text into a dual-mode CLI that serves both humans and automation well. The project added rich terminal rendering, structured JSON and TOON output, better error/reporting surfaces, and a consistent output context that travels through the command dispatcher.

### Delivered capability

- Rich terminal output for high-traffic commands and panels/tables/components.
- Stable JSON output surfaces for agents and machine tooling.
- TOON output support across a broad set of commands.
- Quiet mode semantics that suppress human chatter without breaking robot surfaces.
- Structured error surfaces and output-mode-aware formatting behavior.

### Closed workstreams

- [`beads_rust-3ayr`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-3ayr%22+path%3A.beads%2Fissues.jsonl&type=code) Agent ergonomics & dual-mode CLI
- [`beads_rust-6llm`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-6llm%22+path%3A.beads%2Fissues.jsonl&type=code) Rich Rust integration
- [`beads_rust-2rb9`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-2rb9%22+path%3A.beads%2Fissues.jsonl&type=code) CLI + output-mode compatibility
- [`beads_rust-s9a`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-s9a%22+path%3A.beads%2Fissues.jsonl&type=code) Output formats & JSON schema parity
- [`beads_rust-pzr`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-pzr%22+path%3A.beads%2Fissues.jsonl&type=code) Structured JSON error output

### Representative commits

- [`715a3ef`](https://github.com/Dicklesworthstone/beads_rust/commit/715a3ef) added colored terminal formatting.
- [`a81fa2b`](https://github.com/Dicklesworthstone/beads_rust/commit/a81fa2b) introduced long/pretty output modes with box-drawing connectors.
- [`6a1618c`](https://github.com/Dicklesworthstone/beads_rust/commit/6a1618c) expanded TOON support across count/epic/stale/history/orphans/query.
- [`9565af0`](https://github.com/Dicklesworthstone/beads_rust/commit/9565af0) added TOON structured output for audit/lint/version.
- [`6143daa`](https://github.com/Dicklesworthstone/beads_rust/commit/6143daa) rendered TOON format errors as JSON instead of vague text.
- [`5642445`](https://github.com/Dicklesworthstone/beads_rust/commit/5642445) replaced JSON/TOON panics with graceful serialization failures.

* * *

## 5) Routing, Agent Coordination, and MCP Surfaces Matured

Another long-running theme is that `br` is meant to participate in larger agentic workflows, not just act as a local TODO list. That led to cross-project routing, external dependency syntax, agent-focused commands, MCP surfaces, and better issue-ID resolution semantics.

### Delivered capability

- Cross-project routing and redirect-aware issue dispatch.
- External dependency references like `external:<project>:<capability>`.
- Optional MCP server integration for agent tooling.
- Better agents command UX and server-side filtering fixes.
- Centralized ID resolution and safer mutation flows.

### Closed workstreams

- [`beads_rust-b9o`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-b9o%22+path%3A.beads%2Fissues.jsonl&type=code) Routing + redirects
- [`beads_rust-oqa`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-oqa%22+path%3A.beads%2Fissues.jsonl&type=code) External dependency resolution
- [`beads_rust-26lx`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-26lx%22+path%3A.beads%2Fissues.jsonl&type=code) MCP server integration epic
- [`beads_rust-3e5j`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-3e5j%22+path%3A.beads%2Fissues.jsonl&type=code) Atomic claim guard
- [`beads_rust-nz0`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-nz0%22+path%3A.beads%2Fissues.jsonl&type=code) ID resolution & prefix matching

### Representative commits

- [`4522ca3`](https://github.com/Dicklesworthstone/beads_rust/commit/4522ca3) added external dependency resolution for cross-project coordination.
- [`2195144`](https://github.com/Dicklesworthstone/beads_rust/commit/2195144) added the optional MCP server.
- [`be49fef`](https://github.com/Dicklesworthstone/beads_rust/commit/be49fef) introduced cross-project issue routing.
- [`9b43240`](https://github.com/Dicklesworthstone/beads_rust/commit/9b43240) extended routing to all mutation commands while completing TOON/quiet support.
- [`747e845`](https://github.com/Dicklesworthstone/beads_rust/commit/747e845) improved agents output and refined blocked-cache handling.
- [`94c9138`](https://github.com/Dicklesworthstone/beads_rust/commit/94c9138) centralized issue-ID resolution into shared helpers.

* * *

## 6) Workspace Reliability and Real-World Failure Modeling Became Explicit Engineering Work

The repo moved from “green tests are probably enough” toward a more reality-based reliability model. This includes explicit failure taxonomies, workspace-health contracts, incident bundles, failure corpora, evolution scenarios, replay harnesses, and stronger recovery/doctor behavior.

### Delivered capability

- A documented workspace-health contract and failure taxonomy.
- Incident bundle guidance for field failures.
- Sanitized fixture corpora for corrupted/drifted workspaces.
- Deterministic long-lived evolution scenarios and replay-oriented scaffolding.
- Better doctor diagnostics and broader automatic recovery coverage.
- Non-hermetic smoke coverage for existing-workspace and ambient-environment regressions.

### Closed workstreams

- [`beads_rust-rsei`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei%22+path%3A.beads%2Fissues.jsonl&type=code) Reliability epic
- [`beads_rust-rsei.1.1`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.1.1%22+path%3A.beads%2Fissues.jsonl&type=code) Failure-mode taxonomy
- [`beads_rust-rsei.1.2`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.1.2%22+path%3A.beads%2Fissues.jsonl&type=code) Workspace invariant matrix
- [`beads_rust-rsei.2.1`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.2.1%22+path%3A.beads%2Fissues.jsonl&type=code) Sanitized fixture corpus
- [`beads_rust-rsei.2.3`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.2.3%22+path%3A.beads%2Fissues.jsonl&type=code) Deterministic workspace evolution scenarios
- [`beads_rust-rsei.5.4`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.5.4%22+path%3A.beads%2Fissues.jsonl&type=code) Non-hermetic smoke profile

### Representative commits

- [`1e163ed`](https://github.com/Dicklesworthstone/beads_rust/commit/1e163ed) generalized JSONL recovery to all mutation commands and expanded doctor diagnostics.
- [`5cfc4e0`](https://github.com/Dicklesworthstone/beads_rust/commit/5cfc4e0) hardened diagnostics and documented the workspace health contract.
- [`05dc2ec`](https://github.com/Dicklesworthstone/beads_rust/commit/05dc2ec) added workspace-failure fixtures, dataset registry plumbing, and harder concurrency tests.
- [`046c311`](https://github.com/Dicklesworthstone/beads_rust/commit/046c311) added replay tests and the workspace-evolution framework.
- [`e430d4c`](https://github.com/Dicklesworthstone/beads_rust/commit/e430d4c) introduced database-family snapshots and quarantine support.
- [`5f1da48`](https://github.com/Dicklesworthstone/beads_rust/commit/5f1da48) warned when root `.gitignore` hides `.beads/.gitignore`, a subtle real-world operator trap.

* * *

## 7) Multi-Agent Concurrency and Blocked-Cache Correctness Became a Sustained Hardening Track

A large amount of late history is about making `br` behave sanely under concurrent agent use, especially around blocked-cache maintenance, auto-import, JSONL-only writes, routed workspaces, and read/write contention. This is the part of the history most directly tied to the “don’t tank agent productivity” goal.

### Delivered capability

- Deferred blocked-cache refresh and stale-marker protocols.
- Safer batched status mutations with deferred invalidation.
- Graceful read-path fallbacks when blocked-cache data is stale, malformed, or incomplete.
- Stronger concurrency stress tests around close/update/reopen/read mixes.
- Downgraded concurrent auto-import conflicts from hard stops to warnings where correctness allowed.
- Graceful handling of missing dependencies instead of crashing graph/storage paths.

### Closed workstreams

- [`beads_rust-rsei.3.2`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.3.2%22+path%3A.beads%2Fissues.jsonl&type=code) Broaden multi-process contention scenarios
- [`beads_rust-rsei.4.5`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rsei.4.5%22+path%3A.beads%2Fissues.jsonl&type=code) Fix spurious primary-key errors after JSONL-only writes
- [`beads_rust-3h0h`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-3h0h%22+path%3A.beads%2Fissues.jsonl&type=code) Auto-recover malformed `blocked_issues_cache` schema
- [`beads_rust-s2qp`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-s2qp%22+path%3A.beads%2Fissues.jsonl&type=code) Exclude `in_progress` issues from `ready`
- [`beads_rust-vwuc`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-vwuc%22+path%3A.beads%2Fissues.jsonl&type=code) Fix `changelog --json` output mode
- [`beads_rust-rud3`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rud3%22+path%3A.beads%2Fissues.jsonl&type=code) Preserve absent startup booleans as `None`

### Representative commits

- [`674b9bd`](https://github.com/Dicklesworthstone/beads_rust/commit/674b9bd) added deferred blocked-cache refresh with a stale-marker protocol.
- [`45232f6`](https://github.com/Dicklesworthstone/beads_rust/commit/45232f6) reduced lock contention by deferring blocked-cache refresh for dependency mutations.
- [`afa8d06`](https://github.com/Dicklesworthstone/beads_rust/commit/afa8d06) added stale-marking fallback and update-command error resilience.
- [`ad27f47`](https://github.com/Dicklesworthstone/beads_rust/commit/ad27f47) switched to single-row blocked-cache inserts and added a large-batch regression test.
- [`acedf9d`](https://github.com/Dicklesworthstone/beads_rust/commit/acedf9d) added graceful fallback for all blocked-cache read operations.
- [`30d95b4`](https://github.com/Dicklesworthstone/beads_rust/commit/30d95b4) added the direct close/update/reopen blocked-cache integrity stress test.
- [`4bc6681`](https://github.com/Dicklesworthstone/beads_rust/commit/4bc6681) downgraded concurrent auto-import `SyncConflict` to a warning for concurrent writes.
- [`617572f`](https://github.com/Dicklesworthstone/beads_rust/commit/617572f) made storage/graph paths fall back gracefully on missing dependencies.

* * *

## 8) Performance and Storage Throughput Improved Dramatically

Performance work shows up throughout the history, but by the late-March releases it became a headline capability: lower read/write contention, better query behavior, bulk operation cleanup, and the `frankensqlite` upgrade that materially changed write throughput.

### Delivered capability

- Repeated query- and allocation-level storage optimizations.
- Reduced write contention from read-only command paths.
- Lazy config loading and tighter sync-lock behavior.
- `frankensqlite` upgrades aligned with the project’s concurrency and performance needs.
- Better batch-counting, label filtering, serialization, and cache-maintenance behavior.

### Closed workstreams

- [`beads_rust-220r`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-220r%22+path%3A.beads%2Fissues.jsonl&type=code) Performance & benchmarks epic
- [`beads_rust-1k9`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-1k9%22+path%3A.beads%2Fissues.jsonl&type=code) Performance benchmarks
- [`beads_rust-qy6m`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-qy6m%22+path%3A.beads%2Fissues.jsonl&type=code) Schema/storage parity & validation
- [`beads_rust-6ii1`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-6ii1%22+path%3A.beads%2Fissues.jsonl&type=code) Fix large `fsqlite`-driven test failure batches

### Representative commits

- [`8a8c5f9`](https://github.com/Dicklesworthstone/beads_rust/commit/8a8c5f9) removed N+1 count queries from text-mode list output early in the project.
- [`33335b3`](https://github.com/Dicklesworthstone/beads_rust/commit/33335b3) eliminated write contention from read-only CLI commands.
- [`8a5522f`](https://github.com/Dicklesworthstone/beads_rust/commit/8a5522f) moved blocked-by computation into Rust and reduced allocations.
- [`c059e07`](https://github.com/Dicklesworthstone/beads_rust/commit/c059e07) improved bulk dirty-ID inserts and hardened the sync import pipeline.
- [`39f3e0e`](https://github.com/Dicklesworthstone/beads_rust/commit/39f3e0e) upgraded `frankensqlite` to `v0.1.1` for roughly 100x write performance.
- [`a690d58`](https://github.com/Dicklesworthstone/beads_rust/commit/a690d58) lazy-loaded config, reduced sync-lock contention, and opted out of checkpoint-on-close where appropriate.

* * *

## 9) The Final `v0.1.29` to `v0.1.30` Wave Expanded the Command Surface While Cleaning Up Edge Cases

The most recent tagged history is not just bug-fixing. It materially expanded stats, graph, lint, blocked, count, stale, epic, and list behavior while also cleaning up agent-facing edge cases, pagination semantics, startup flags, and output correctness.

### Delivered capability

- Larger stats surface with more aggregate metrics.
- Better graph/stale/lint/count/blocked/epic behavior and coverage.
- Correct paginated list semantics and cleaner jq examples for agents.
- Cleaner startup override semantics and sync/config file writing behavior.
- Better `ready`, `show`, `changelog`, and plain-text sync behavior in edge cases.

### Closed workstreams

- [`beads_rust-54tl`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-54tl%22+path%3A.beads%2Fissues.jsonl&type=code) Force JSON for `orphans --robot`
- [`beads_rust-iqj9`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-iqj9%22+path%3A.beads%2Fissues.jsonl&type=code) Fix routed `show --format json/toon`
- [`beads_rust-oap8`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-oap8%22+path%3A.beads%2Fissues.jsonl&type=code) Restore sync import processed count in plain text
- [`beads_rust-s2qp`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-s2qp%22+path%3A.beads%2Fissues.jsonl&type=code) Exclude `in_progress` from `ready`
- [`beads_rust-vwuc`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-vwuc%22+path%3A.beads%2Fissues.jsonl&type=code) Fix `changelog --json`
- [`beads_rust-rud3`](https://github.com/Dicklesworthstone/beads_rust/search?q=%22beads_rust-rud3%22+path%3A.beads%2Fissues.jsonl&type=code) Preserve absent startup bools

### Representative commits

- [`ac4ff74`](https://github.com/Dicklesworthstone/beads_rust/commit/ac4ff74) delivered a major stats/storage expansion.
- [`4703dff`](https://github.com/Dicklesworthstone/beads_rust/commit/4703dff) and [`b634768`](https://github.com/Dicklesworthstone/beads_rust/commit/b634768) continued the stats expansion with more aggregates.
- [`76b8dd4`](https://github.com/Dicklesworthstone/beads_rust/commit/76b8dd4) improved graph/stale behavior, MCP surfaces, storage, and tests together.
- [`c4f861c`](https://github.com/Dicklesworthstone/beads_rust/commit/c4f861c) expanded lint and added a dedicated E2E lint suite.
- [`3126725`](https://github.com/Dicklesworthstone/beads_rust/commit/3126725) expanded count/stale coverage.
- [`e273d58`](https://github.com/Dicklesworthstone/beads_rust/commit/e273d58) broadened list output modes and config/storage support.
- [`36a5ff8`](https://github.com/Dicklesworthstone/beads_rust/commit/36a5ff8) fixed pagination by applying offset after client-side filtering.
- [`e3a00e3`](https://github.com/Dicklesworthstone/beads_rust/commit/e3a00e3) switched atomic config writes to PID-scoped temp files.

* * *

## Notes for Agents

- The fastest way to understand a historical capability is:
  1. Read the section summary here.
  2. Open the linked bead/workstream if you need intent and acceptance criteria.
  3. Open the linked commits if you need the actual implementation slice.
- The most important late-history themes are:
  - sync safety and no-data-loss guarantees
  - blocked-cache correctness under concurrency
  - workspace recovery / doctoring / real-world failure coverage
  - agent-facing output correctness across JSON, TOON, quiet, and routing modes
