# Migration fixture regenerations

Existing fixtures must not be regenerated during ordinary development. If a
fixture is exceptionally repaired with `cargo xtask migration-fixture
--regenerate-existing NNN --reason "..."`, the generator appends the selected
fixture, its new SHA-256, and the supplied reason below.

Regeneration entries are appended below this line.
