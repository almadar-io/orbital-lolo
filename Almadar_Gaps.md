<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-lolo — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-LOLO-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

Next code: `G-ORBITAL-LOLO-002`

- **G-ORBITAL-LOLO-001** [mechanical] — `cargo test -p orbital-lolo` stops at `tests/contracts_cache.rs` (`unchanged_packages_load_once_and_a_new_lolo_is_seen`): its fake package set installs no `@almadar/core`, so `discover` answers `Language(NotInstalled { package: "@almadar/core" })` and the later test targets never run in a plain suite run (2026-11-04). Owners: the test's fixture (install a fake core beside the fake std, as orbital_test_support does elsewhere). Prevention rung: none (test fixture). Root cause: one-off.
