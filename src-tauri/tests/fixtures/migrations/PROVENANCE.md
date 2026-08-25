# Migration fixture provenance

These database files are immutable upgrade evidence. Git history was checked on
2026-08-24 to determine when each file first appeared and whether it arrived in
the same commit as its migration.

| Fixtures | Fixture introduction | Migration introduction | Finding |
| --- | --- | --- | --- |
| `001`–`006` | `501e316` (2026-08-13) | `6812a8e`–`d1119b4` (2026-05-20 through 2026-05-21) | These are prefix reconstructions created when the corpus was introduced, not databases retained from the releases that originally shipped schemas 1–6. Their bytes have not changed since they were committed, but they must not be described as era-native databases. |
| `007` | `4305d4b` (2026-08-23) | `4305d4b` (2026-08-23) | Created and committed with its migration; unchanged since. |
| `008`–`009` | `e0518dd` (2026-08-23) | `e0518dd` (2026-08-23) | Created and committed with their migrations; unchanged since. |
| `010`–`011` | `623b61b` (2026-08-24) | `623b61b` (2026-08-24) | Created and committed with their migrations; unchanged since. |

`SHA256SUMS` pins the bytes established by that history. The migration tests
verify every checksum on every CI run.

SQLite serialization is not byte-reproducible for this generator. A same-process
experiment on 2026-08-24 created the current schema twice from identical
migrations and seed data; the resulting bytes differed. The sqlx migration
ledger includes runtime metadata such as `installed_on` and `execution_time`, so
generated databases are artifacts rather than reproducible builds. The byte lock
therefore has no tolerance: it verifies the exact committed artifact and never
tries to compare it with freshly generated output.
