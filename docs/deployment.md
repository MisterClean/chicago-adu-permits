# Native release deployment

The CI workflow checks both supported development platforms, then builds Linux x86-64
executables on Ubuntu 24.04. Only a successful push to `main` publishes a `build-<commit>`
release. The release contains two executables, the checked map-renderer assets, checksums, and a schema manifest. It never
contains runtime configuration, secrets, databases, sessions, or deployment inventories.
The Rust toolchain is pinned so lint changes cannot unexpectedly block a release.

## Host configuration

Supply private host configuration using `deploy/updater.example.toml` as a template.
Run `adu-bot-updater --config <private-config>` periodically as an unprivileged service.
Its install directory and the runtime state directory must already exist and be writable
by its user. Keep the normal worker's release directory read-only using service sandboxing.
The updater does not update itself: updater and service-policy changes require an explicit
operator installation. A `deploy-paused` file in the install directory pauses updates.

Keep immutable release directories, credentials/configuration, and persistent state separate.
Set an absolute `state_dir` and `require_existing_state = true` in production. Initialize or
restore state explicitly before starting workers; never ship a database with a release.
Keep the same Bluesky DID to preserve the account's delivery namespace. Transfer secrets
through an encrypted administrative connection into protected files, outside source control.
The CLI does not automatically load environment files; the service manager must load them.

Petit can invoke `systemctl start --wait` for a dedicated oneshot worker. A narrowly scoped
authorization rule should permit only the required unit names and start operation. Petit owns
the schedule and records command failures; systemd supplies the worker identity, environment,
resource limits, timeout, and filesystem isolation. Do not also enable the standalone worker
timer. Run every 15 minutes; the app's six-hour ingestion interval limits full scans while
allowing timely queue draining and retries. Give backup and health checks their own jobs.
Validate resource limits with a real image preview on the target Linux host before enabling
publication. All normal CLI operations participate in the deployment lock.

## Activation and migration

The updater accepts only the successful release for current `main`, verifies the downloaded
binary hash, and defers when a runtime command holds the deployment lock. It creates a
SQLite-consistent backup and tests the candidate migration and read-only integrity check on
a separate copy. Backups pause new publication upon restore. The baseline, event keys and
sent delivery identities must survive migration unchanged.

`adu-bot check` opens the database read-only, checks schema, integrity, foreign keys and the
required baseline, and does not initialize state or contact external services. `check --health`
also fails on stale ingestion, paused/disabled publishing, and held/failed deliveries.

Add future upgrades to `src/migrations.rs` as ordered SQL steps and advance `CURRENT`.
Existing runtime commands refuse an old schema until it has been explicitly migrated. The
migration command requires an existing database and writer lock; operators must also exclude
normal runs with the deployment lock for the complete migration/activation operation.
All pending SQL steps commit together. Test both successful upgrades and failure rollback.

Ordinary code deployments do not change the live database. When the schema changes, the
updater blocks runtime execution, migrates, checks preserved identities and integrity, then
atomically replaces the `current` symlink. If migration or activation is interrupted, the
`deployment-blocked` marker keeps normal runs stopped for operator recovery. Do not simply
remove that marker without checking schema compatibility and the active release.

## Rollback and retention

The updater keeps the current and previous executables. Pause automatic deployment before
an operator rollback, acquire the deployment lock, verify the earlier binary's `check`
against the current database, and atomically switch the symlink only if compatible. Never
restore an old database automatically when reverting code: that can discard post receipts.
Incompatible schemas require a forward fix or a reviewed, paused restore and reconciliation.

Take daily consistent backups, retain 30 days, and copy them off-host using the operator's
backup system. Rotate dated deployment backups as well. Check disk capacity before migrations.
Monitor Petit failures and `check --health`, plus queue age and corrections from `status`.
Deployment success alone does not prove a future external API call will succeed.

## Scorecard worker

Schema 2 adds map locations and reply deliveries. The updater verifies the seven renderer assets against the manifest and installs them under `/opt/adu-bot/current/map-renderer` when that is the configured install path. The updater does not update itself, so install the new `adu-bot-updater` binary through the existing operator procedure **before** it attempts to activate a schema-2 release. Keep `[scorecards].enabled = false` during this code and schema upgrade. A full post-migration ingest is required before a scorecard can use the new map locations; it does not reset or resend the root queue.

The example scorecard systemd unit and Petit job are separate from the native announcement worker. Point `[scorecards].renderer_dir` to `/opt/adu-bot/current/map-renderer` and set `node_bin` and `chrome_bin` to installed absolute executable paths. The renderer needs Chrome software WebGL and network access to OpenFreeMap/OpenMapTiles and Cook County. A 1536 MiB `MemoryMax` only constrains the worker; it does not supply RAM. Measure a source-backed `scorecards preview` on the intended host with the service's limits before enabling the schedule. If the host lacks the capacity, provision a renderer-capable host rather than enabling this unit there. Do not run two workers against independent copies of the same SQLite database.

The Petit schedule starts the scorecard service at minutes 12, 27, 42, and 57 UTC, offset from the main schedule. Install the new unit and Petit job through the normal operator process, run the explicit activation step in [operations](operations.md#scorecard-activation-and-recovery), and then enable the job. Monitor its exit status, structured journal events, `scorecards list`, and `check --health`. Reply failures never require replaying the parent announcement.
