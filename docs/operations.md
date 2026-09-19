# Operations

## Initial deployment

Use one Linux host with persistent local disk, one unprivileged `adu-bot` user, and one state directory. No inbound port is needed. Do not share SQLite over a network filesystem or run replicas against independent copies of the same account history.

Build and test a Linux release on an appropriate build host. Install the binary as `/usr/local/bin/adu-bot`; create the service user with your distribution's account tools. Copy `config.example.toml` to `/etc/adu-bot/config.toml` and set `state_dir = "/var/lib/adu-bot"`. Keep publishing disabled. Install the service/timer from `deploy/` in `/etc/systemd/system/` and reload systemd.

Keep `/etc/adu-bot/credentials.env` mode 0600, owned by root (systemd reads it before dropping privileges). The optional app-password file must be accessible to the service user and no other users. Use a dedicated bot account and a Bluesky app password, never a primary password. The state directory is mode 0700; persisted session tokens use mode 0600 and are excluded from database backups.

Run the initial ingest as the service user, inspect `status --json`, and preview several known applications. The initial 471 qualifying rows observed during implementation were suppressed; your initial count may differ. Never reset the database to clear a queue.

After the authenticated test gate, set the production DID and credentials, explicitly set `publish_enabled = true`, then enable `adu-bot.timer`. Use `journalctl -u adu-bot.service` for structured ingestion/errors. The service's 64 MiB cap is provisional until validated on your Linux host. Its 13-minute outer timeout bounds the invocation even if a network call extends beyond the application's 12-minute work budget.

The timer wakes every 15 minutes with jitter. Successful ingestion makes the next scan due six hours later; failed scans are retried from page one on a later invocation. `run` still attempts safe reconciliation if ingestion fails; ingestion does not depend on Bluesky availability. Work is paced across invocations without sleeping. The default timer therefore normally sends at most one new announcement each wakeup; change the wakeup interval if higher throughput is needed while preserving the 60-second send minimum.

## Monitoring

Check exit codes and status after each scheduled run. Alert on failed runs, unknown statuses, held/failed deliveries, growing pending queue age, corrections, and successful ingestion older than 24 hours. An unchanged source revision is different from a failed poll. Application-level issues persist as audit history; issue counts are cumulative. Inspect current unknown statuses separately from historical issue counts.

The source revision is `rowsUpdatedAt`, not an update SLA. Changes between polls can be missed. A large count decrease fails closed and retains staging for inspection; there is no automatic override switch. Confirm the cause before changing the contract or threshold in a reviewed release.

## Review and recovery

```sh
adu-bot --config /etc/adu-bot/config.toml queue list
adu-bot --config /etc/adu-bot/config.toml queue inspect 'chicago:j4h8-ug9m:123:preapproval-first-observed:v1'
adu-bot --config /etc/adu-bot/config.toml queue approve 'chicago:j4h8-ug9m:123:preapproval-first-observed:v1' --reason 'Verified current preapproval and requested units'
```

Approval selects the current version as reviewed evidence. It can deliberately release a baseline backfill. It cannot approve absent, nonqualifying, or invalid/future-dated records. A first administrative adjustment requires explicit approval. An attempted unresolved delivery must be retried/reconciled before its evidence can change. Suppression prevents further sends but permits reconciliation of an already ambiguous attempt.

`queue retry` preserves record identity and frozen JSON. If reconciliation finds a different record, the conflict stays held. Do not overwrite, allocate another key, or directly alter a sent receipt. Automatic correction posts, deletions, and edits are outside v1. Investigate operator-visible correction issues against any sent URI.

Auth failures pause the adapter. Correct credentials or identity routing first. If the refresh token is invalid, stop the timer and remove only the protected `bluesky-session.json` to permit fresh app-password authentication; never remove the database. Run `adapter check` to verify authentication without posting, then use `adapter resume --reason '...'`. Account changes must be deliberate: a different configured DID creates a distinct delivery namespace and may queue pending events for that account. Review before enabling it.

## Backups and restore

Install `deploy/adu-bot-backup` as `/usr/local/libexec/adu-bot-backup` and its service/timer alongside the main units. The daily job uses the SQLite backup API, checks integrity, retains dated backups for 30 days, and pauses publishing **inside each backup**. Copy backups off the host through your existing backup system. Backups contain project-address history, not session tokens. Securely handle credentials separately.

The writer lock can make a backup job fail if a run overlaps; monitor failures and rerun after the active command exits. Never copy a live SQLite file as a substitute for `backup`.

To restore:

1. Stop the main and backup timers/services. Preserve the current state directory and incident logs.
2. Keep configuration publishing disabled. Place the selected consistent backup at `state_dir/adu.sqlite3`; do not overwrite the only copy of the latest database. Restore owner/mode and a valid session or app-password source separately.
3. Check `PRAGMA integrity_check` and `PRAGMA foreign_key_check` with SQLite. Run `status --json` and a complete ingest while publishing remains disabled.
4. Reconcile every potentially sent record after the backup timestamp using the newer database, receipts, and authoritative account records. An old backup cannot infer TIDs that were allocated after it. If the lost interval cannot be reconciled, remain paused; do not send blindly.
5. For known frozen attempted deliveries, `queue retry` and `publish` can reconcile while the database posting pause blocks new sends. A `RecordNotFound` result while paused becomes held; review it before release.
6. Only after resolving the lost interval, use `adapter resume --reason 'Restored and reconciled through ...'`, review held work, explicitly enable publishing, and restart the timer.

Do not prune event keys, versions, or sent receipts. Deleting state destroys duplicate protection.

## Authenticated release gate

The posting portion of this gate requires an explicitly authorized dedicated test account and has **not been run**. App-password login and returned DID/PDS verification passed with `adapter check`, which creates no posts. Mock HTTP tests do not certify actual PDS write semantics.

Use a separate state directory and test account DID. Authorize and review a single application event or a controlled test fixture. Verify:

- Login, returned DID/PDS routing, access-token expiry, refresh with the refresh JWT, and persistence of rotated tokens.
- One real `putRecord` creation with a valid TID and explicit `swapRecord: null`.
- Authoritative `getRecord` returning the identical frozen record and a stable URI/CID.
- A same-key retry and same-key/different-content guarded conflict without overwriting the record.
- Actual Bluesky rendering, Unicode facets, and the linked single-application City record.
- Restart after remote acceptance but before the local receipt commits; reconciliation must mark the existing record sent without a second identity.

Never run these tests on the production account without explicit operator intent. Production starts with a suppressed baseline even after the test account passes.
