# Architecture and policy

## Source contract

The preapproval source is `j4h8-ug9m` on `data.cityofchicago.org`. Application identity is `(dataset_id, id)`; identical addresses do not merge applications. No predecessor dataset is automatically combined with it. Schema 2 adds a separate issued-permit candidate scan from `ydr8-5enu`; its source identity is the permit row ID and its public identity is the permit number.

`normalize::FIELDS` defines the query allowlist and expected metadata types. The selected source payload is retained per observed content version, alongside normalized fields. Missing and explicit null compare equally; integral decimal strings/numbers canonicalize without floating point. Optional invalid values generate issues and omit claims. Required invalid identifiers reject the entire scan. Added unused columns are ignored; any changed/missing selected column type rejects the scan.

SHA-256 hashes cover named canonical fields and contract version 1. Each successful run also records a digest of its ordered `(application_id, content_hash)` sequence. No fetch time, platform state, or Socrata internal row ID affects content identity.

Permit scans also verify source schema, revision, ordered keyset pages, and count agreement before promotion. They retain only matching fields, excluding permit contacts. Permit links preserve method, score, status, and review history. [Permit policy](permit-announcements.md) specifies automatic and review-required links. A permit event uses `(preapproval application ID, permit number)`, and its root and map reply each have a separate frozen delivery identity.

Floating source timestamps retain their original naive strings. The baseline date uses America/Chicago as an explicit operational assumption. Observation and delivery timestamps are UTC. Invalid or future source dates hold qualifying decisions. Posts use calendar dates, not invented timezones.

## Snapshot promotion

An OS advisory lock serializes commands for one state directory. All commands currently acquire this lock, including readers. SQLite uses a 2 MiB cache target, rollback journaling, FULL synchronization, foreign keys, and a five-second busy timeout.

Every scan creates an ingest-run record, stages bounded pages, verifies strict increasing IDs, count agreement, schema stability, and unchanged `rowsUpdatedAt`. Exact page-size multiples require the final empty request. Empty snapshots after a nonempty current state and decreases exceeding `max(10, floor(previous_count × 0.05))` fail closed. Failed staging is retained seven days; run metadata is retained indefinitely.

Promotion reads staging/current observations through cursors in `BEGIN IMMEDIATE`. Current rows, content/presence versions, event decisions, configured-account delivery rows, baseline state, and successful-run marker commit together. On startup, abandoned fetching runs become failed. The safeguards do not create a transactional snapshot guarantee on Socrata.

Versions are ordered observations, not unique hashes. A → B → A yields three versions; absence retains the prior payload with `present=false`. The latest version at or before a successful run reconstructs the last observed state.

## Event rules

The immutable event key is `chicago:j4h8-ug9m:{id}:preapproval-first-observed:v1`. It is independent of dates, templates, content hashes, and platforms.

| Source status or transition | Decision |
| --- | --- |
| Baseline `Pre-Certified` or `Pre-Certified: admin adjust` | Suppressed |
| Known nonapproved → `Pre-Certified` | Pending unless historical/invalid/future date |
| First seen already `Pre-Certified` | Pending only with valid action date on/after baseline |
| First qualifying administrative adjustment | Held |
| Unknown → pre-certified | Held |
| Canceled, Cancelled, Denied | No event |
| Submitted, In Review, Resubmitted, Request Additional Docs, Upload affordability receipt, Upload missing information | No event |
| Any other exact status | Unknown issue; relevant decisions held |
| Existing event, including reversal/reapproval | Preserve same event; no second announcement |

Classification trims surrounding whitespace only. A missing action date is acceptable for a genuinely observed known-status transition. Suppressed baseline events may be released only through explicit reviewed backfill approval. Changed post facts or absence produce persistent issues and hold unsent work. Sent corrections are operator-visible; automatic edits/deletes/correction posts are outside v1.

## Outbox and platform boundary

Events store initial evidence separately from operator-approved evidence. Deliveries are unique by `(event_key, platform, account DID)`. Delivery identity and immutable JSON are committed before a first post write, followed by a durable `sending` state and attempt log. Restarts reconcile attempted work first, even if the application later disappears or source data becomes stale.

Template v2 prepares a native JPEG with project-address Street View, plain-language unit type and Chicago typography/colors. It uploads the image before freezing the blob reference, alt text and dimensions into the post. An interrupted preparation may leave an unreferenced blob, but cannot create a post. Preparation failures are deferred with zero post attempts, so evidence remains reviewable. Once frozen, retries do not fetch Street View or upload another image. Legacy frozen v1 text-only records retain their payloads. Text previews/dry runs remain offline; explicit image previews fetch Street View without Bluesky authentication.

Bluesky uses a 13-character, persisted monotonic TID and a guarded create via `putRecord`. Structural equality with the frozen record yields a sent receipt. Different content holds a conflict. Only an explicit PDS `RecordNotFound` permits another guarded write. Failed reads never mean absence. Sent receipts are terminal even if someone deletes the remote post.

The account adapter handles login, session persistence/refresh, returned DID verification, PDS routing, HTTP error classification, and receipts. Shared queue code handles freshness/evidence gates, attempts, pacing, review, and scheduling. The `Publisher` interface includes identity allocation and template versioning. A second platform implements these alongside its own rendering and recovery semantics; do not assume TID or create-if-absent support on Threads.

Retry delays start at 1, 5, 15, and 60 minutes, then 6 hours, plus up to 30 seconds of jitter. Server retry/reset hints can increase delays. After ten unsuccessful attempts work remains failed for review. Auth failures pause the adapter. Validation/conflict errors hold a delivery. Ambiguous prior writes retain their identities throughout review.

No process sleeps to pace sends. At the default 15-minute wakeup interval, normally one new post is sent per invocation. Ten is an upper bound on processed due deliveries, not a throughput promise; reconciliation does not require a write interval. Excess work stays queued.

## Limitations

Current-state polling observes only states present at successful scans. `action_date` is the current status date, not proof of the original approval date. Administrative-adjustment semantics and the City's timestamp timezone remain unverified. Building-permit detection, cross-dataset address matching, Threads, a dashboard, and property-level consolidation are deferred. The dataset does not distinguish attic and garden apartments; conversion-unit announcements use “ADU apartment.” Street View can predate the application and may not show a rear coach house; it is context, not evidence of completed construction.

A backup older than a remote write cannot independently reconstruct that lost delivery identity. Restores are paused and require external reconciliation; no distributed exactly-once claim is made.
