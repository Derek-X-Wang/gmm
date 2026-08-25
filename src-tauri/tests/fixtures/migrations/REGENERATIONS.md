# Migration fixture regenerations

Existing fixtures must not be regenerated during ordinary development. If a
fixture is exceptionally repaired with `cargo xtask migration-fixture
--regenerate-existing NNN --reason "..."`, the generator appends the selected
fixture, its new SHA-256, and the supplied reason below.

Pull-request CI requires this file to change whenever `SHA256SUMS` changes. That
makes a reason mandatory on the ordinary review path; it cannot prevent a
determined person from editing the fixture, checksum, and record together, and
is not intended to be tamper-proof.

Regeneration entries are appended below this line.
