# Chicago ADU preapproval bot

A small Rust command-line service that watches Chicago's [Additional Dwelling Unit Preapproval Applications](https://data.cityofchicago.org/Buildings/Additional-Dwelling-Unit-Preapproval-Applications/j4h8-ug9m/about_data) and queues one Bluesky announcement per newly qualifying application.

**This feed reports Department of Housing preapprovals, not building permits.** Every announcement says that a building permit is still required. One application may request multiple ADUs. The bot reports requested units, never completed homes.

## Start with publishing disabled

Requires Rust 1.88 or later and a C compiler for bundled SQLite. CI tests stable Rust on Linux and macOS. Build the release binary away from a memory-constrained server.

```sh
cargo build --release --locked
cp config.example.toml config.toml
./target/release/adu-bot --config config.toml ingest
./target/release/adu-bot --config config.toml status --json
./target/release/adu-bot --config config.toml preview --application-id 955045
./target/release/adu-bot --config config.toml publish --dry-run
```

The first complete scan establishes the baseline and suppresses existing preapprovals. An empty dry run immediately afterward is expected. Preview renders a selected application's current facts without queueing or posting it. Neither preview nor dry run requires Bluesky credentials. Previewing a nonapproved application is for template inspection only; the queue enforces eligibility.

Use a dedicated Bluesky account and a revocable **app password**. Configure the account DID and optionally its handle in `[bluesky]`. Supply the password through `BLUESKY_APP_PASSWORD` or a protected file, and optionally set `SOCRATA_APP_TOKEN`. The CLI does not automatically load `.env` files. The PDS advertised in authenticated session identity is used for record operations. Sessions are persisted privately with rotated access and refresh tokens. Run `adu-bot --config config.toml adapter check` to verify credentials without posting.

Publishing remains disabled until `publish_enabled = true` is explicitly configured. Complete the [authenticated acceptance gate](docs/operations.md#authenticated-release-gate) on an authorized test account before enabling production. No public posts were made during implementation.

## Commands

`--config PATH` is accepted with every command. Omit it to use defaults, including `./state` and disabled publishing.

| Command | Purpose |
| --- | --- |
| `ingest` | Fetch and validate a complete source snapshot, then atomically promote it |
| `run` | Ingest when due, then handle due deliveries if publishing is enabled |
| `publish --dry-run` | Render pending event evidence; no authentication, sends, or receipt changes |
| `publish` | Attempt due deliveries, subject to configuration and safety gates |
| `status --json` | Source freshness, baseline, changes, issues, event and delivery counts |
| `preview --application-id ID` | Render current application facts without queueing |
| `queue list` | List decisions, including historical suppressions and holds |
| `queue inspect EVENT_KEY` | Inspect initial/current/approved evidence, frozen records, reviews, and receipts |
| `queue approve EVENT_KEY --reason TEXT` | Explicitly approve current qualifying evidence |
| `queue suppress EVENT_KEY --reason TEXT` | Prevent further new sends for that event |
| `queue retry EVENT_KEY --reason TEXT` | Reconcile an attempted delivery using its existing identity and payload |
| `backup DESTINATION` | SQLite-consistent backup with publishing paused in the backup |
| `adapter check` | Verify app-password authentication without posting; reports public DID/PDS only |
| `adapter resume --reason TEXT` | Clear account/database pause after operator checks |

A queue review never sends a post. Holds do not disappear when a source row or mapping changes. For work that has never been attempted, use `approve` to release a hold; use `retry` for attempted work. Retry never changes the frozen record or TID. Already sent deliveries are never re-created.

## Design

```text
Socrata allowlisted pages → SQLite staging → validated atomic promotion
                                           ├─ current applications + versions
                                           ├─ platform-independent events + reviews
                                           └─ per-platform/account delivery outbox
                                                          ↓
                                           deterministic renderer → Bluesky PDS
```

- Blocking HTTP, 100-row keyset pages, a 2 MiB decompressed response cap, before/after count and schema/revision checks. Partial scans never change the baseline or current state.
- Exact status classification, namespaced application IDs, stable first-preapproval event keys, and conservative review of adjustments, backfills, unknown prior states, and invalid/future dates.
- Full observed version history, including A → B → A and absence/reappearance. Polling cannot reconstruct transitions between scans.
- Immutable delivery records with persisted monotonic TIDs; guarded `putRecord` includes explicit `swapRecord: null`. Uncertain results are reconciled on the authoritative PDS before another send.
- Credentials are separate from the database. No names, personal mailing addresses, maps, or geocoding enter the ingestion allowlist. Project addresses are public post facts.
- Deterministic templates generate text; no LLM, web server, queue server, or external post-generation service is needed.

`events` knows nothing about Bluesky. `publish::Publisher` separates platform formatting, remote identity reconciliation, and sending from shared queue rules. Adding Threads requires its own adapter and recovery research; it is not implemented. Record identity allocation also belongs to the adapter; other platforms need not use Bluesky TIDs.

See [architecture and policy](docs/architecture.md), [operations](docs/operations.md), and [validation and measured resources](docs/validation.md).

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

Tests use synthetic data and local HTTP servers. They cover state/review policy, real SQLite rollback and process kills, lock exclusion, simulated disk exhaustion, HTTP failure/retry behavior, authentication rotation, UTF-8 facets, and backup integrity. Tests never contact a social account.

Only source, migrations, synthetic tests, configuration examples, and operating documentation belong in Git. State databases, backups, credentials, local configuration, logs, and build products are ignored. No research dumps, pasted handoff files, or machine-specific paths are required by the application.
