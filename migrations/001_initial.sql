CREATE TABLE ingest_runs (
 id INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, ended_at INTEGER,
 status TEXT NOT NULL CHECK(status IN ('fetching','success','failed')),
 requests INTEGER NOT NULL DEFAULT 0, revision_before TEXT, revision_after TEXT,
 count_before INTEGER, count_after INTEGER, fetched_rows INTEGER NOT NULL DEFAULT 0,
 digest TEXT, failure TEXT, changed_rows INTEGER NOT NULL DEFAULT 0, missing_rows INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE source_state (
 dataset_id TEXT PRIMARY KEY, baseline_run INTEGER NOT NULL REFERENCES ingest_runs(id), baseline_date TEXT NOT NULL,
 last_successful_run INTEGER NOT NULL REFERENCES ingest_runs(id), contract_version INTEGER NOT NULL CHECK(contract_version=1),
 posting_paused INTEGER NOT NULL DEFAULT 0 CHECK(posting_paused IN (0,1))
);
CREATE TABLE staged_applications (
 run_id INTEGER NOT NULL REFERENCES ingest_runs(id), application_id TEXT NOT NULL,
 observation TEXT NOT NULL CHECK(json_valid(observation)), content_hash TEXT NOT NULL,
 PRIMARY KEY(run_id, application_id)
);
CREATE TABLE applications (
 dataset_id TEXT NOT NULL, application_id TEXT NOT NULL, version INTEGER NOT NULL,
 observation TEXT NOT NULL CHECK(json_valid(observation)), content_hash TEXT NOT NULL,
 present INTEGER NOT NULL CHECK(present IN (0,1)), first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
 changed_run INTEGER NOT NULL REFERENCES ingest_runs(id), PRIMARY KEY(dataset_id, application_id)
);
CREATE TABLE application_versions (
 dataset_id TEXT NOT NULL, application_id TEXT NOT NULL, version INTEGER NOT NULL,
 observed_run INTEGER NOT NULL REFERENCES ingest_runs(id), content_hash TEXT NOT NULL,
 observation TEXT NOT NULL CHECK(json_valid(observation)), present INTEGER NOT NULL CHECK(present IN (0,1)),
 previous_version INTEGER, changed_fields TEXT NOT NULL CHECK(json_valid(changed_fields)),
 PRIMARY KEY(dataset_id, application_id, version),
 FOREIGN KEY(dataset_id,application_id) REFERENCES applications(dataset_id,application_id)
);
CREATE TABLE events (
 event_key TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, application_id TEXT NOT NULL,
 initial_evidence_version INTEGER NOT NULL, evidence_version INTEGER NOT NULL,
 detection_kind TEXT NOT NULL, observed_at INTEGER NOT NULL, action_date TEXT,
 disposition TEXT NOT NULL CHECK(disposition IN ('pending','held','suppressed')),
 reason TEXT NOT NULL, rule_version INTEGER NOT NULL DEFAULT 1,
 FOREIGN KEY(dataset_id,application_id,initial_evidence_version) REFERENCES application_versions(dataset_id,application_id,version),
 FOREIGN KEY(dataset_id,application_id,evidence_version) REFERENCES application_versions(dataset_id,application_id,version)
);
CREATE TABLE deliveries (
 id INTEGER PRIMARY KEY, event_key TEXT NOT NULL REFERENCES events(event_key), platform TEXT NOT NULL, account_id TEXT NOT NULL,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','prepared','sending','retry','sent','held','failed','suppressed')),
 record_key TEXT, record_json TEXT CHECK(record_json IS NULL OR json_valid(record_json)), record_hash TEXT,
 template_version INTEGER, evidence_version INTEGER, attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0,
 remote_uri TEXT, remote_cid TEXT, last_error TEXT, sent_at INTEGER,
 UNIQUE(event_key, platform, account_id), UNIQUE(platform, account_id, record_key)
);
CREATE TABLE review_actions (
 id INTEGER PRIMARY KEY, event_key TEXT NOT NULL REFERENCES events(event_key), action TEXT NOT NULL, reason TEXT NOT NULL,
 at INTEGER NOT NULL, prior_disposition TEXT NOT NULL, new_disposition TEXT NOT NULL, evidence_version INTEGER NOT NULL
);
CREATE TRIGGER review_no_update BEFORE UPDATE ON review_actions BEGIN SELECT RAISE(ABORT,'review log is append only'); END;
CREATE TRIGGER review_no_delete BEFORE DELETE ON review_actions BEGIN SELECT RAISE(ABORT,'review log is append only'); END;
CREATE TABLE issues (
 id INTEGER PRIMARY KEY, application_id TEXT, run_id INTEGER REFERENCES ingest_runs(id), event_key TEXT REFERENCES events(event_key),
 kind TEXT NOT NULL, detail TEXT NOT NULL, at INTEGER NOT NULL, UNIQUE(run_id,application_id,kind)
);
CREATE TABLE delivery_attempts (
 id INTEGER PRIMARY KEY, delivery_id INTEGER NOT NULL REFERENCES deliveries(id), at INTEGER NOT NULL,
 outcome TEXT NOT NULL, detail TEXT
);
CREATE TABLE adapter_state (
 platform TEXT NOT NULL, account_id TEXT NOT NULL, paused INTEGER NOT NULL DEFAULT 0,
 last_send INTEGER, last_identity_clock INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(platform,account_id)
);
CREATE INDEX delivery_due ON deliveries(platform,account_id,state,next_attempt);
CREATE INDEX version_run ON application_versions(observed_run);
CREATE INDEX event_application ON events(dataset_id,application_id);
PRAGMA user_version=1;
