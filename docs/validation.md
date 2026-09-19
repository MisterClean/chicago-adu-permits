# Validation record

Implementation checks performed September 19, 2026. These results describe this development host, not an untested production deployment.

## Automated checks

- `cargo fmt --check` and strict Clippy across all targets/features passed.
- `cargo test --locked`: 38 passing tests (including the subprocess crash-test helper).
- Release application and validation examples compiled successfully.
- Synthetic state tests cover baseline suppression, unchanged replay, A → B → A, absence/reappearance, duplicate addresses with distinct IDs, exact status mapping, adjustments/backfills/unknown states, invalid/future dates, Unicode/long text, zero units, explicit review, immutable sent records, staleness, and per-account/platform uniqueness.
- Real SQLite tests cover foreign keys, an append-only review log, transaction rollback on event/delivery write failure, writer-lock exclusion, backup integrity and a paused restore, and `SQLITE_FULL` during staging via a page-count limit.
- A subprocess is killed during fetching and inside the actual promotion transaction. Restart preserves zero partial baseline/current/history/events and marks the interrupted run failed.
- Local HTTP integration tests cover bounded keyset requests and a final empty page, field allowlisting, malformed/oversized JSON, schema drift, explicit-null guarded writes, structural reconciliation, wrong DID rejection, expired-token recovery using the refresh JWT, rotated session file permissions, 429/Retry-After, 5xx, validation/conflict responses, and timeout after remote acceptance.
- Outbox tests reopen SQLite after an ambiguous write and reconcile the exact same stored record/TID; stale source data does not prevent discovering an already successful write. Conflicts never overwrite, and deleted sent records are not recreated.

## Live read-only checks

The source schema and exact status values were rechecked against the City's metadata/resource APIs. Anonymous resource GET was available.

| Result | Observed |
| --- | --- |
| Current applications | 502 |
| Plain pre-certified | 461 |
| Administrative adjustments | 10 |
| Initial suppressed events | 471 |
| Initial versions | 502 |
| Baseline public deliveries | 0 (no account configured) |
| Replay changes/new versions/new events | 0 / 0 / 0 |
| Unknown current statuses / normalization issues | 0 / 0 |
| Source revision | `1789776105` |
| Requests per complete scan | 10 (two metadata, two count, six pages) |
| Qualifying previews validated | 471 |
| Generated single-record link | HTTP 200, exactly application `955045` |

No applicant/owner fields were fetched by the production adapter. No live source dumps, state databases, or machine-specific research artifacts are committed.

## Release resource measurements

Host: ARM64 macOS (`aarch64-apple-darwin`). Compiler: rustc 1.94.0, LLVM 21.1.8. Bundled SQLite: 3.53.2. Release profile uses thin LTO. RSS is `/usr/bin/time -l` maximum resident set size, which macOS reports in **bytes**, converted below to MiB. Single runs, warm developer host; not statistical performance claims.

| Workload | Wall time | Peak RSS | Database size |
| --- | --- | --- | --- |
| Live 502-row baseline, including network/TLS | 3.60 s | 17.61 MiB | 1,986,560 bytes after replay |
| Live unchanged replay, including network/TLS | 2.63 s | 16.50 MiB | Same database |
| Synthetic 50,000-row baseline + replay, no HTTP/TLS | 7.43 s total | 17.14 MiB | 187,342,848 bytes |

The synthetic ingestion phases measured 3.57 s for baseline and 3.40 s for replay inside the executable. Synthetic data is generated in 100-row pages; the whole fixture is never held in memory. The database keeps selected raw and normalized payloads in current state and history, explaining its larger disk footprint than the research spike.

The observed live workload meets the provisional ≤32 MiB RSS target on this Mac. **Operation under an enforced 64 MiB Linux memory cap has not been tested.** A low RSS measurement is not proof of behavior under a cgroup cap. Validate the supplied systemd units on the actual host before production.

Reproduce without committing generated state:

```sh
cargo build --release --locked --examples --bin adu-bot
# Use a fresh directory outside the repository for each measurement.
/usr/bin/time -v target/release/examples/benchmark /tmp/adu-benchmark 50000
# macOS uses /usr/bin/time -l instead of -v.
target/release/examples/check_previews /path/to/existing/state
```

On the Linux deployment host, repeat the synthetic benchmark in a transient systemd unit with `MemoryMax=64M`, inspect memory peak/OOM status, and test the actual timer/service. Run the application as its production service user. Do not benchmark with a production state directory.

## Gates still open

- Real posting, token refresh, guarded same-key behavior, conflict, authoritative record reconciliation, and final rendering still require the authorized test-account gate. App-password login and returned DID/PDS verification passed using `adapter check`; no public writes were attempted.
- Actual Linux/systemd execution and the enforced 64 MiB memory-cap benchmark.
- Filesystem-level full-disk/power-loss testing on the deployment host. SQLite page exhaustion and process kills passed locally, but do not simulate every storage failure.
- Multi-day source cadence/identifier stability observation and live account/PDS migration.

Publishing is disabled by default. See the [operating runbook](operations.md) for enabling it only after the required checks.
