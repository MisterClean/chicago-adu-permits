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
./target/release/adu-bot --config config.toml preview --application-id 955045 --image state/preview.jpg
./target/release/adu-bot --config config.toml publish --dry-run
```

The first complete scan establishes the baseline and suppresses existing preapprovals. An empty dry run immediately afterward is expected. Preview renders a selected application's current facts without queueing or posting it. Neither preview nor dry run requires Bluesky credentials. Previewing a nonapproved application is for template inspection only; the queue enforces eligibility.

Use a dedicated Bluesky account and a revocable **app password**. Configure the account DID and optionally its handle in `[bluesky]`. Supply the password through `BLUESKY_APP_PASSWORD` or a protected file, and optionally set `SOCRATA_APP_TOKEN`. The CLI does not automatically load `.env` files. The PDS advertised in authenticated session identity is used for record operations. Sessions are persisted privately with rotated access and refresh tokens. Run `adu-bot --config config.toml adapter check` to verify credentials without posting.

Publishing remains disabled until `publish_enabled = true` is explicitly configured. See the [authenticated acceptance gate](docs/operations.md#authenticated-release-gate) before enabling production.

## Announcement cards

Post text starts with “New ADU preapproved” and dynamically includes the positive `adu_applying_for` count proposed for the property, the type supported by the `conversion_unit` / `coach_house` flags (Conversion, Coach house, or both), and calendar days from `submission_date` to `action_date`. Timing is included only for `Pre-Certified` records with valid dates in chronological order; administrative adjustments may have a later action date and omit it. Missing or invalid details are omitted. The post ends with a linked “Data Portal Record.” For example:

```text
New ADU preapproved

2 ADUs proposed for the property.
Type: Conversion.

Preapproved 32 days after submission.

Data Portal Record
```

Application details and the building-permit distinction remain in the card and alt text. Every newly prepared post includes an application-rendered **3200 × 4000** portrait JPEG, detailed alt text, the project address, requested home count, ward, preapproval date, and a link to the single city record. The card headline uses plain language: **New coach house**, **ADU apartment(s)**, or **Coach house + apartments**. The current dataset has no project description or reliable floor designation, so the bot does not guess garden/basement or attic apartments. Unknown/invalid type flags use **Additional home(s)** on the card. All posts distinguish housing preapproval from a building permit.

Cards use bundled Big Shoulders Bold and Roboto, black/white, Chicago flag blue (`#41B6E6`), and star red (`#E4002B`), following the [Chicago typography](https://design.chicago.gov/typography/) and [color guidance](https://design.chicago.gov/basics/). The feed is labeled unofficial. The lower panel shows Google Street View requested by project address, keeping the entire photograph and Google's attribution visible. It is street-facing context, not a rendering of the proposed unit.

Set `GOOGLE_MAPS_API_KEY` or `[media].google_api_key_file` to a protected file containing the key. The CLI does not load `.env` automatically. Street View API access and billing must be enabled. Google's standard API returns up to 640 pixels per side; the card uses a 640 × 360 photo enlarged to its panel, while text renders at native resolution. Missing imagery, credentials, or upload errors stop that announcement instead of sending text alone.

The renderer selects the highest JPEG quality within [Bluesky's 2,000,000-byte / 4,000-pixel limits](https://github.com/bluesky-social/social-app/blob/main/src/lib/constants.ts). It uploads the image before freezing the post's blob reference, alt text and aspect ratio in the durable outbox. Retries reuse that frozen record; existing already-prepared version-1 records retain their original text-only payloads.

`preview --image PATH.jpg` writes the JPEG and sibling `PATH.alt.txt` without posting or authenticating to Bluesky. It requires Google credentials; ordinary text previews and `publish --dry-run` remain offline. New rendering dependencies are native Rust libraries; fonts are embedded in the binary.

## Profile artwork

The selected second-post neighborhood and ward map designs are available in the
[map preview](tools/map-preview/README.md), with reproducible browser rendering
and a dated public sample. Automatic second-post publishing is not integrated.

The [profile avatar](assets/profile/README.md) pairs a red Chicago star with a blue
coach house outline. Its SVG master and 1024 × 1024 PNG upload copy are generated
from the same paths with the existing Rust graphics libraries:

```sh
cargo run --locked --example generate_avatar
```

## Commands

`--config PATH` is accepted with every command. Omit it to use defaults, including `./state` and disabled publishing.

| Command | Purpose |
| --- | --- |
| `ingest` | Fetch and validate a complete source snapshot, then atomically promote it |
| `run` | Ingest when due, then handle due deliveries if publishing is enabled |
| `publish --dry-run` | Render pending event evidence; no authentication, sends, or receipt changes |
| `publish` | Attempt due deliveries, subject to configuration and safety gates |
| `status --json` | Source freshness, baseline, changes, issues, event and delivery counts |
| `preview --application-id ID [--image PATH.jpg]` | Render current application facts and optionally a Street View card without queueing |
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

Source, migrations, synthetic tests, configuration examples, operating documentation, and documented public design samples belong in Git. State databases, backups, credentials, local configuration, logs, and build products are ignored. No research dumps, pasted handoff files, or machine-specific paths are required by the application.
