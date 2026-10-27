# MVP validation — 2026-10-26

Validated locally on Linux x86_64, AMD EPYC 9654 96-Core Processor, Rust 1.98.1. Shared development host; timings are a small workload measurement, not a capacity guarantee.

## Automated results

- `cargo test --locked`: **11 integration tests passed**.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `cargo fmt --check`: passed.
- Release binary built and fixture replay/server executed successfully.
- Browser checks passed at 1280px and 390px widths: two workers, source health, completion/error state, raw provenance, refresh, no JavaScript exceptions or horizontal overflow.
- Restart validation includes an injected transaction failure and a real SIGKILL/restart with no duplicate source records.

The tests cover CRLF and split lines, interleaved workers, unknown fields, malformed progress, explicit-ID result deduplication, rollback of events/state/checkpoint, competing writers, rename rotation with an old-inode tail, truncation, same-size rewrite, oversized records, missing files, cursor pagination, bad API inputs, and separate CLI replay processes.

## Measured fixture workload

Two synthetic workers, 1,002 initial records / 44,977 bytes, 200 ms poll interval. After initial ingestion, idle CPU was sampled for 10.00 seconds from `/proc/PID/stat`; RSS from `/proc/PID/status`. Five appended records were timed to visibility through SQLite (20 ms polling measurement resolution).

| Measurement | Observed |
| --- | --- |
| Resident memory | 8.49 MiB |
| Idle CPU, fraction of one core | 0.30% |
| Complete-record visibility | 182.8–204.1 ms |
| Two polling intervals | 400 ms |
| SIGKILL/restart without duplicate records | Passed |

These fixture results meet the proposed 50 MiB, 1% idle-core, and two-interval visibility targets for this small run. The observer was measured separately from the HTTP server/browser. See [raw resource measurements](resource-measurement.json) and [browser check output](browser-validation.json); rerun with `python3 scripts/measure.py` after a release build.

## Unverified boundary

No live mprime process or captured home-machine logs were available. Therefore this release does **not** claim compatibility with a particular mprime version, actual work progress, or live-machine restart validation. Supported grammar and all synthetic fields are documented in `supported-formats.md`. GitHub CI configuration is included but has not run remotely.
