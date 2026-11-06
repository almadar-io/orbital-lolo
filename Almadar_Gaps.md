<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-lolo — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-LOLO-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

Next code: `G-ORBITAL-LOLO-003`

- **G-ORBITAL-LOLO-002** [mechanical] — `tests/contracts_cache.rs` `unchanged_packages_load_once_and_a_new_lolo_is_seen` fails in a fresh monorepo install with `Language(NotInstalled { package: "@almadar/core" })`. The root `package.json` names `@almadar/core` only under `pnpm.overrides`, not as a dependency, so `pnpm install --frozen-lockfile` creates no root `node_modules/@almadar/core` link. The test passes in the main checkout only because a link from an Oct 22 install is still there. Seen 2026-11-06 in the `gpu-floor` worktree. Owner: the test, which should name its language data instead of reaching the workspace root, or the root manifest. Repro: a fresh worktree, `pnpm install --frozen-lockfile --ignore-scripts`, then `cargo test -p orbital-lolo --test contracts_cache`. Prevention rung: none (environment). Root cause: periphery.

- **G-ORBITAL-LOLO-001** [mechanical] — `cargo test -p orbital-lolo` stops at `tests/contracts_cache.rs` (`unchanged_packages_load_once_and_a_new_lolo_is_seen`): its fake package set installs no `@almadar/core`, so `discover` answers `Language(NotInstalled { package: "@almadar/core" })` and the later test targets never run in a plain suite run (2026-11-04). Owners: the test's fixture (install a fake core beside the fake std, as orbital_test_support does elsewhere). Prevention rung: none (test fixture). Root cause: one-off.
