CREATE TABLE permit_runs (
 id INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, ended_at INTEGER,
 status TEXT NOT NULL CHECK(status IN ('fetching','success','failed')),
 revision_before TEXT, revision_after TEXT, count_before INTEGER, count_after INTEGER,
 fetched_rows INTEGER NOT NULL DEFAULT 0, failure TEXT
);
CREATE TABLE staged_permits (
 run_id INTEGER NOT NULL REFERENCES permit_runs(id), source_id TEXT NOT NULL,
 observation TEXT NOT NULL CHECK(json_valid(observation)),
 PRIMARY KEY(run_id,source_id)
);
CREATE TABLE permits (
 source_id TEXT PRIMARY KEY, permit_number TEXT NOT NULL,
 observation TEXT NOT NULL CHECK(json_valid(observation)), present INTEGER NOT NULL CHECK(present IN (0,1)),
 first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
 UNIQUE(permit_number)
);
CREATE TABLE permit_state (
 id INTEGER PRIMARY KEY CHECK(id=1), baseline_run INTEGER NOT NULL REFERENCES permit_runs(id),
 last_successful_run INTEGER NOT NULL REFERENCES permit_runs(id),
 posting_paused INTEGER NOT NULL DEFAULT 0 CHECK(posting_paused IN (0,1))
);
CREATE TABLE permit_matches (
 application_id TEXT NOT NULL, source_id TEXT NOT NULL REFERENCES permits(source_id),
 method TEXT NOT NULL, score INTEGER NOT NULL, status TEXT NOT NULL CHECK(status IN ('proposed','confirmed','rejected')),
 reason TEXT NOT NULL, first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
 PRIMARY KEY(application_id,source_id)
);
CREATE INDEX permit_matches_source ON permit_matches(source_id);
CREATE TABLE permit_match_reviews (
 id INTEGER PRIMARY KEY, application_id TEXT NOT NULL, source_id TEXT NOT NULL,
 action TEXT NOT NULL CHECK(action IN ('confirm','reject')), reason TEXT NOT NULL,
 at INTEGER NOT NULL, prior_status TEXT NOT NULL, new_status TEXT NOT NULL,
 FOREIGN KEY(application_id,source_id) REFERENCES permit_matches(application_id,source_id)
);
CREATE TRIGGER permit_match_review_no_update BEFORE UPDATE ON permit_match_reviews BEGIN SELECT RAISE(ABORT,'permit match review log is append only'); END;
CREATE TRIGGER permit_match_review_no_delete BEFORE DELETE ON permit_match_reviews BEGIN SELECT RAISE(ABORT,'permit match review log is append only'); END;
CREATE TABLE permit_event_evidence (
 event_key TEXT PRIMARY KEY REFERENCES events(event_key), source_id TEXT NOT NULL REFERENCES permits(source_id),
 permit_number TEXT NOT NULL, observation TEXT NOT NULL CHECK(json_valid(observation)),
 application_observation TEXT NOT NULL CHECK(json_valid(application_observation)),
 match_method TEXT NOT NULL
);
CREATE TABLE permit_replies (
 id INTEGER PRIMARY KEY, event_key TEXT NOT NULL REFERENCES events(event_key),
 platform TEXT NOT NULL, account_id TEXT NOT NULL,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','prepared','sending','retry','sent','held','failed','suppressed')),
 record_key TEXT, record_json TEXT CHECK(record_json IS NULL OR json_valid(record_json)), record_hash TEXT,
 attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0,
 remote_uri TEXT, remote_cid TEXT, last_error TEXT, sent_at INTEGER,
 UNIQUE(event_key,platform,account_id), UNIQUE(platform,account_id,record_key)
);
CREATE INDEX permit_replies_due ON permit_replies(platform,account_id,state,next_attempt);
CREATE TABLE permit_reply_attempts (
 id INTEGER PRIMARY KEY, reply_id INTEGER NOT NULL REFERENCES permit_replies(id),
 at INTEGER NOT NULL, outcome TEXT NOT NULL, detail TEXT
);
PRAGMA user_version=2;
