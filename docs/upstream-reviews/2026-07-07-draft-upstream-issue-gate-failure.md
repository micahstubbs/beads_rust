# DRAFT upstream issue — Dicklesworthstone/beads_rust

Status: SUPERSEDED — already filed upstream by the concurrent session before
this draft could be submitted (discovered 2026-07-07, treasury-n626):

- Issue #375 (this report): https://github.com/Dicklesworthstone/beads_rust/issues/375
- PR #373 (user_version): https://github.com/Dicklesworthstone/beads_rust/pull/373
- PR #374 (self-collision): https://github.com/Dicklesworthstone/beads_rust/pull/374

This session's duplicate branches remain on the fork as
`pr-upstream/wal-aware-user-version` and `pr-upstream/import-self-collision`
(equivalent content; the wal branch also ports the read-only-path fallback,
and the collision branch carries a second, cross-issue regression test not in
#374). Delete once #373/#374 merge:
`git push origin --delete pr-upstream/wal-aware-user-version pr-upstream/import-self-collision`

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
