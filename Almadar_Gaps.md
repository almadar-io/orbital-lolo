<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-lolo — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-LOLO-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

Next code: `G-ORBITAL-LOLO-005`


- **G-ORBITAL-LOLO-003** [mechanical] — Five orbital-lolo tests fail on main (2026-11-06), seen only with `--no-fail-fast` (an earlier failing target stops the run first): `multi_entity` `map_of_entity_rows_validates_compiles_and_types_its_reads` and `a_read_of_a_field_the_entity_lacks_is_refused_through_the_map` (`ORB_X_OPERATOR_NULL_OPERAND` now refuses `object/get` over a map read that can be missing, which the fixtures do on purpose); `tuple_type` `tuple_record_paths_are_inferred_without_accepting_missing_fields` ("unknown tuple member still rejected"); `unlower_round_trip` `corpus_io_behaviors_sample` and `corpus_unlower_of_lower_is_equivalent_over_every_file` (4 almadar-behaviors experience organisms: "reference orbital with imports not shared by every orbital"). Owner per test: the null-operand rule or the fixtures, the tuple inference, the unlower printer. Repro: `cargo test -p orbital-lolo --test <target> --no-fail-fast`. Prevention rung: (3) the verifiers (run every crate's suite to the end at a checkpoint). Folds G-ORBITAL-LOLO-004 (duplicate, 2026-11-07): [mechanical] — `cargo test -p orbital-lolo --test tuple_type tuple_record_paths_are_inferred_without_accepting_missing_fields` fails at tuple_type.rs:179 ("unknown tuple member still rejected"), reported 2026-11-07 by the iOS/Android parity session; likely from that day's union/record-model commits. Rung: (3) the crate suite. Severity: medium. Root cause: one-off.


