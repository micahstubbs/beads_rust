# DRAFT upstream issue — Dicklesworthstone/beads_rust

Status: DRAFT, not yet filed. Companion to the two PR branches
`pr-upstream/wal-aware-user-version` and `pr-upstream/import-self-collision`
(treasury-n626).

---

**Title:** Release gate `workspace_failure_replay_manifest_expectations_hold_on_fresh_copies` fails on the `corrupt_db_text` fixture on current main

**Body:**

`workspace_failure_replay_manifest_expectations_hold_on_fresh_copies` fails on
the `corrupt_db_text` fixture on current `main`, reproduced with an unmodified
upstream build.

What we observe:

- The harness copies the WAL-bearing `beads/` payload for the
  `corrupt_db_text` fixture, then `doctor --repair` exits **7 after a
  successful rebuild**, so the manifest expectation fails.
- Reproduced with a baseline binary built from pure `upstream/main` (no local
  patches): same exit 7 on the same payload.
- Reproduced on both fsqlite **0.1.13** and **0.1.14** (Linux, x86_64).

Timeline note: the v0.2.17 release gates passed on 2026-07-04, and we first
hit this on 2026-07-06 with no relevant local changes — so the trigger looks
environmental or data-dependent (fixture payload interacting with WAL state?)
rather than a recent code regression.

Happy to provide the full replay output or bisect further if useful.

---

Filing notes (not part of the issue body):

- Evidence recorded in `docs/upstream-reviews/2026-07-06-dicklesworthstone-main.md`
  (addendum) and commit b68b6048.
- Our v0.2.18 fork release shipped with `skip_reliability_gates` and this
  reason recorded; link the issue back into that doc once filed.
