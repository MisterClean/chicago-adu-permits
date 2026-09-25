CREATE TABLE map_locations (
 dataset_id TEXT NOT NULL, application_id TEXT NOT NULL,
 observed_run INTEGER NOT NULL REFERENCES ingest_runs(id),
 latitude REAL, longitude REAL,
 PRIMARY KEY(dataset_id,application_id),
 FOREIGN KEY(dataset_id,application_id) REFERENCES applications(dataset_id,application_id),
 CHECK ((latitude IS NULL AND longitude IS NULL) OR (latitude IS NOT NULL AND longitude IS NOT NULL))
);
CREATE TABLE reply_deliveries (
 id INTEGER PRIMARY KEY,
 parent_delivery_id INTEGER NOT NULL UNIQUE REFERENCES deliveries(id),
 enqueued_at INTEGER NOT NULL DEFAULT 0, enqueue_reason TEXT NOT NULL DEFAULT 'automatic',
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','prepared','sending','retry','sent','held','failed','suppressed')),
 snapshot_json TEXT CHECK(snapshot_json IS NULL OR json_valid(snapshot_json)), snapshot_hash TEXT,
 record_key TEXT, record_json TEXT CHECK(record_json IS NULL OR json_valid(record_json)), record_hash TEXT,
 template_version INTEGER, attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0,
 remote_uri TEXT, remote_cid TEXT, last_error TEXT, sent_at INTEGER,
 UNIQUE(record_key)
);
CREATE TABLE reply_attempts (
 id INTEGER PRIMARY KEY,
 reply_delivery_id INTEGER NOT NULL REFERENCES reply_deliveries(id),
 at INTEGER NOT NULL, outcome TEXT NOT NULL, detail TEXT
);
CREATE INDEX reply_due ON reply_deliveries(state,next_attempt);
CREATE TABLE scorecard_state (
 id INTEGER PRIMARY KEY CHECK(id=1), activated_at INTEGER NOT NULL
);
