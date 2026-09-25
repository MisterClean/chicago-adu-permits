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

## Announcement cards (2026-09-19)

Template v2 adds native 3200 × 4000 cards, Google Street View, Big Shoulders/Roboto, plain-language headlines, and accessible image embeds. The earlier memory measurements above describe text-only v1. The service now specifies `MemoryMax=256M`; an enforced Linux cgroup measurement is still pending.

- `cargo test --locked`: 45 tests passed, including type fallback, missing/invalid flags, requested counts, alt text, full-resolution JPEG decoding, byte-limit/quality selection, binary upload, expired-token recovery, and image preparation failure without a text-only send.
- `cargo fmt --check`, strict all-target Clippy, release build, and `git diff --check` passed.
- All 471 current qualifying city observations passed offline text/facet validation.
- Visual inspection covered coach-house and conversion-apartment cards. Their JPEGs are 1,871,556 bytes at quality 94 and 1,900,115 bytes at quality 93. Both retain 3200 × 4000 dimensions and all Google attribution.
- A release-mode coach-house publish completed in 4.44 seconds with 161,153,024 bytes (153.7 MiB) peak RSS on this ARM64 Mac. These are single-run measurements, not Linux-cap certification.
- Live testing exposed and fixed the existing refresh request body: `refreshSession` expects an empty POST, and `ExpiredToken` can arrive with HTTP 400. Regression coverage now checks both.
- User-authorized posts on the configured account exercise upload, guarded post creation, and browser rendering. Authoritative `getRecord` is checked against frozen JSON, including image blob, alt text and aspect ratio. Live destructive conflict/crash simulations remain outside this design review.

Bluesky limits: https://github.com/bluesky-social/social-app/blob/main/src/lib/constants.ts and https://github.com/bluesky-social/atproto/blob/main/lexicons/app/bsky/embed/images.json.
Chicago brand: https://design.chicago.gov/typography/ and https://design.chicago.gov/basics/.
Google's standard photo size: https://developers.google.com/maps/documentation/streetview/usage-and-billing.

Live design-review posts (the user may delete these after review):

- Coach house, application 955045: https://bsky.app/profile/chiadupreapprovals.bsky.social/post/3mvv7ibtz6ow5
- Apartment, application 954542: https://bsky.app/profile/chiadupreapprovals.bsky.social/post/3mvv7jui6pmvs

Both authoritative records matched their frozen outbox records exactly. Both post records include a 3200 × 4000 image and nonempty alt text (617 / 752 characters). Regular local publishing remains disabled; the temporary test configuration was disabled after the two sends. The rest of the baseline remains suppressed.

## Simplified copy revision (template v3)

Following the live review, post text is exactly `New ADU preapproved\n\nData Portal Record`, with the final label linked to the same application-specific data portal URL. The card masthead “CHICAGO / MORE HOMES” is removed; conversion headlines now say “ADU APARTMENT” (plural when appropriate). Application details and the permit qualification remain on the card and in alt text.

All 45 tests, strict Clippy, and the release build pass. The revised Sheffield card is 3200 × 4000, 1,878,629 bytes, JPEG quality 93. A separate user-authorized design-review post was created and read back against its frozen local review record without changing the production event receipts: https://bsky.app/profile/chiadupreapprovals.bsky.social/post/3mvv7z2e5ry22. Automatic publishing remains disabled.

## Ward scorecard implementation (2026-09-24)

The schema-2 migration was exercised against a SQLite-consistent copy of the production database, entirely on the development machine. `integrity_check` passed, and all 471 prior root delivery identities and states matched a before/after digest. A complete City scan on that copy promoted 512 current applications in run 19, with 511 valid coordinate pairs and one missing/invalid pair. The current cohort included one application explicitly reporting **zero** requested ADUs; scorecard aggregation keeps that application and its zero, while an unknown quantity remains an error.

The read-only `scorecards preview 954542` on that copied state fetched Ward 43 geometry and rendered two 2160 × 2160 JPEGs (1,367,752 and 628,918 bytes). The snapshot reported 16 requested ADUs across 13 Ward 43 applications, tied at rank 11, out of 502 requested ADUs across 477 citywide applications as of September 24. Both output images were visually inspected. Each of the six missed September 24 application IDs (`955117`, `955240`, `955259`, `955744`, `956988`, `957212`) also produced both images and a ward snapshot; the dense Ward 32 pair was visually inspected. A second run from a temporary directory containing only the seven release-packaged renderer assets completed with no MapLibre errors, which checks the installed asset layout rather than relying on development `node_modules` or font fallback paths.

Local tests cover the migration, source-map isolation from root content versions, rank ties, zero quantities, missing coordinates, activation that excludes older roots, and reply reconciliation after an ambiguous write. Strict Clippy, `cargo test --locked`, a release build of both binaries, JavaScript syntax checks, and `git diff --check` passed. The renderer has been checked on macOS with headless Chrome; Linux Chrome/WebGL, the sample systemd memory limits, and live Bluesky reply publication remain untested. The production host observed during investigation has only 412 MiB of physical RAM, so the separate Chrome worker must not be enabled there without a measured capacity solution.
