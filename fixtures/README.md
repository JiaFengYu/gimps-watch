# Fixture provenance

`basic-run/` is **synthetic input authored for this project**, not a captured run or evidence that an exponent was tested. Exponents, residues, assignment UID, percentages, errors, and timestamps are illustrative. It is distributed under this repository's MIT license.

Expected replay: 9 source records, 2 worker projections, 1 unsupported-line diagnostic, 1 explicit-ID completion key from 2 copied JSON records, and 1 ambiguous text-completion record. Worker 2 retains its last progress and a separate error observation. Replaying into the same database ingests 0 new records.

The parser's field-to-source mapping is in `docs/supported-formats.md`. Home-machine logs were unavailable at implementation time. Before declaring a client version supported, capture its actual output (removing private fields), record version/launch mode/source file and worker mapping, and add independent expected events here.
