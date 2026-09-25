# Operations

## Initial deployment

Use one Linux host with persistent local disk, one unprivileged `adu-bot` user, and one state directory. No inbound port is needed. Do not share SQLite over a network filesystem or run replicas against independent copies of the same account history.

Build and test a Linux release on an appropriate build host. Install the binary as `/usr/local/bin/adu-bot`; create the service user with your distribution's account tools. Copy `config.example.toml` to `/etc/adu-bot/config.toml` and set `state_dir = "/var/lib/adu-bot"`. Keep publishing disabled. Install the service/timer from `deploy/` in `/etc/systemd/system/` and reload systemd.

Keep `/etc/adu-bot/credentials.env` mode 0600, owned by root (systemd reads it before dropping privileges). The optional app-password file must be accessible to the service user and no other users. Use a dedicated bot account and a Bluesky app password, never a primary password. The state directory is mode 0700; persisted session tokens use mode 0600 and are excluded from database backups.

Run the initial ingest as the service user, inspect `status --json`, and preview several known applications. The initial 471 qualifying rows observed during implementation were suppressed; your initial count may differ. Never reset the database to clear a queue.

Provide `GOOGLE_MAPS_API_KEY` in the credentials environment or a protected `[media].google_api_key_file`; use a key with Street View Static API enabled. `preview --application-id ID --image /path/card.jpg` verifies fetching, rendering, and alt text without posting. The Google key is never included in post records or logged request URLs. Missing imagery and network errors defer the announcement; the bot does not silently publish text alone.

After the authenticated test gate, set the production DID and credentials, explicitly set `publish_enabled = true`, then enable `adu-bot.timer`. Use `journalctl -u adu-bot.service` for structured ingestion/errors. The service's 256 MiB cap accommodates native 3200 × 4000 rendering; validate it on your Linux host (a local macOS image preparation/upload attempt measured about 156 MiB peak RSS). Its 13-minute outer timeout bounds the invocation even if a network call extends beyond the application's 12-minute work budget.

The timer wakes every 15 minutes with jitter. Successful ingestion makes the next scan due six hours later; failed scans are retried from page one on a later invocation. `run` still attempts safe reconciliation if ingestion fails; ingestion does not depend on Bluesky availability. Work is paced across invocations without sleeping. The default timer therefore normally sends at most one new announcement each wakeup; change the wakeup interval if higher throughput is needed while preserving the 60-second send minimum.

## Monitoring

Check exit codes and status after each scheduled run. Alert on failed runs, unknown statuses, held/failed deliveries, growing pending queue age, corrections, and successful ingestion older than 24 hours. An unchanged source revision is different from a failed poll. Application-level issues persist as audit history; issue counts are cumulative. Inspect current unknown statuses separately from historical issue counts.

The source revision is `rowsUpdatedAt`, not an update SLA. Changes between polls can be missed. A large count decrease fails closed and retains staging for inspection; there is no automatic override switch. Confirm the cause before changing the contract or threshold in a reviewed release.

## Scorecard activation and recovery

The second-post worker is independent of the announcement worker and is off by default. Schema 2 stores a map location alongside each current application and adds a reply outbox without changing root delivery identities. After migration, complete an `ingest` before previewing or sending scorecards: the scorecard refuses a mixture of old rows and newly mapped rows. Keep `[scorecards].enabled = false` while checking the maps and the renderer host. `scorecards preview APPLICATION_ID --output-dir /path/to/new/dir` uses the latest complete source scan for a currently qualifying application, fetches its official ward boundary, and writes `n5.jpg`, `wc.jpg`, both alt text files, post text, and the source snapshot. It does not need a sent parent, authenticate, or post. The output directory must not contain files of those names.

Install Node 20+ and Chrome with working software WebGL on the renderer host. Confirm that its physical memory and process limits can finish a real preview with room for the ingestion worker; the scorecard service's 1536 MiB limit is a cap, not provisioned memory. Use a larger or separate renderer host if the current host cannot meet that requirement. Rendering requires outbound tile, glyph, sprite, and Cook County boundary access. Failures leave the reply queued with an error and retry time. Map images are checked for 2160 × 2160 dimensions and Bluesky's 2,000,000-byte limit before upload.

To activate automatic replies, set the production DID, `publish_enabled = true`, `[scorecards].enabled = true`, and the release's absolute `renderer_dir`. Run `scorecards run` once **before the next announcement send** to establish the activation time; this first invocation does not queue older sent roots. Then enable the separate Petit scorecard schedule. A new sent root is queued at the next scorecard run. The worker handles one reply per invocation and obeys the shared minimum send interval. `scorecards list` shows all reply states; `status --json` reports counts and oldest queue age. `check --health` includes held/failed replies and flags an enabled reply queue older than 24 hours. Watch `scorecard_sent`, `scorecard_held`, and `scorecard_delivery_error` in the scorecard service journal.

Historical roots are deliberately excluded from automatic queueing. After verifying that each parent exists on the configured account and that its current application still belongs to the scorecard cohort, queue the missed September 24 announcements individually with a reason:

```sh
adu-bot --config /etc/adu-bot/config.toml scorecards enqueue 955117 --reason 'Reviewed September 24 missed reply'
adu-bot --config /etc/adu-bot/config.toml scorecards enqueue 955240 --reason 'Reviewed September 24 missed reply'
adu-bot --config /etc/adu-bot/config.toml scorecards enqueue 955259 --reason 'Reviewed September 24 missed reply'
adu-bot --config /etc/adu-bot/config.toml scorecards enqueue 955744 --reason 'Reviewed September 24 missed reply'
adu-bot --config /etc/adu-bot/config.toml scorecards enqueue 956988 --reason 'Reviewed September 24 missed reply'
adu-bot --config /etc/adu-bot/config.toml scorecards enqueue 957212 --reason 'Reviewed September 24 missed reply'
```

`enqueue` never posts by itself and is idempotent for a parent. Review preview images and text first, then let the scheduled worker send them one at a time. Verify each resulting URI is a reply whose root and parent both match the original announcement. If a reply is held or failed, use `scorecards inspect REPLY_ID` to examine its `last_error`, frozen record, and attempt history before `scorecards retry REPLY_ID --reason TEXT`. Retry preserves any frozen record key and payload. Never queue another parent announcement to repair its reply. If a source value becomes unknown or the focus pin fails the ward-boundary check, correct or review the source evidence before retrying; the worker will not invent a count or map coordinate.

On a host too small to run Chrome, use a consistent **copy** of its post-migration database on a renderer-capable machine to run `scorecards preview APPLICATION_ID --output-dir DIR` for each reviewed backfill. Transfer each complete preview directory into a private location readable by the production service user. After `scorecards enqueue` returns a reply ID, run `scorecards run-prepared REPLY_ID --input-dir DIR` on the production host. This explicit one-time command works while `[scorecards].enabled = false`, but still requires `publish_enabled = true`, a verified parent receipt on the PDS, and an unpaused adapter. On first preparation it computes the production source snapshot; later retries reuse that frozen snapshot. It requires exact agreement with `snapshot.json`, text, alt text, and both valid 2160 × 2160 JPEGs before uploading. If the production source snapshot changed after the copy, repeat the preview from a fresh copy. Run one reply at a time with at least the configured minimum send interval, and inspect each receipt; the command exits with an error unless the reply is `sent`.

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

The full release gate below remains incomplete. The user-authorized card review on 2026-09-19 exercised live image upload, guarded creation, authoritative record equality, token refresh, and browser rendering on the configured account. Live conflict/crash simulations have not been run. Mock HTTP tests do not certify all actual PDS write semantics.

Use a separate state directory and test account DID. Authorize and review a single application event or a controlled test fixture. Verify:

- Login, returned DID/PDS routing, access-token expiry, refresh with the refresh JWT, and persistence of rotated tokens.
- One real `putRecord` creation with a valid TID and explicit `swapRecord: null`.
- Authoritative `getRecord` returning the identical frozen record and a stable URI/CID.
- A same-key retry and same-key/different-content guarded conflict without overwriting the record.
- Actual Bluesky rendering, Unicode facets, and the linked single-application City record.
- Restart after remote acceptance but before the local receipt commits; reconciliation must mark the existing record sent without a second identity.

Never run these tests on the production account without explicit operator intent. Production starts with a suppressed baseline even after the test account passes.
