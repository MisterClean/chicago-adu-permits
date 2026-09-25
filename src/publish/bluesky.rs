use super::{DeliveryError, DeliveryIdentity, Publisher, Receipt, Reconciliation};
use crate::{
    config::{Config, secure_url},
    maps,
    media::{self, PostImage},
    normalize::Observation,
    permits::{Permit, WardSummary},
    render,
    store::now,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::SystemTime,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    did: String,
    access_jwt: String,
    refresh_jwt: String,
    pds: String,
}
struct Response {
    status: u16,
    value: Value,
    retry_after: Option<i64>,
}
pub struct Bluesky {
    config: Config,
    agent: ureq::Agent,
    session: Option<Session>,
    verified: bool,
    recovered: bool,
}
impl Bluesky {
    pub fn new(config: &Config) -> Result<Self> {
        ensure!(
            config.bluesky.did.is_some(),
            "configure bluesky.did before publishing"
        );
        Ok(Self {
            config: config.clone(),
            agent: config.agent(),
            session: None,
            verified: false,
            recovered: false,
        })
    }
    /// Verify authentication and return public identity only; never creates a post.
    pub fn check_auth(&mut self) -> Result<Value> {
        self.recovered = false;
        self.authenticate()?;
        let session = self
            .session
            .as_ref()
            .context("missing authenticated session")?;
        Ok(json!({"authenticated": true, "did": session.did, "pds": session.pds}))
    }
    /// Upload the finished JPEG before freezing the post's immutable blob reference.
    pub fn attach_image(&mut self, record: &mut Value, image: &PostImage) -> Result<()> {
        ensure!(
            !image.bytes.is_empty() && image.bytes.len() <= media::MAX_IMAGE_BYTES,
            "invalid image size"
        );
        let blob = self.upload_blob(&image.bytes)?;
        record["embed"] = image.embed(blob);
        render::validate(record)?;
        Ok(())
    }
    pub fn attach_scorecard_images(
        &mut self,
        record: &mut Value,
        images: &[(Vec<u8>, String)],
    ) -> Result<()> {
        ensure!(images.len() == 2, "scorecard needs two maps");
        let mut embeds = Vec::with_capacity(2);
        for (bytes, alt) in images {
            ensure!(
                !bytes.is_empty() && bytes.len() <= media::MAX_IMAGE_BYTES,
                "invalid map image size"
            );
            ensure!(!alt.is_empty() && alt.len() <= 2000, "invalid map alt text");
            let blob = self.upload_blob(bytes)?;
            embeds.push(json!({"image":blob,"alt":alt,"aspectRatio":{"width":2160,"height":2160}}));
        }
        record["embed"] = json!({"$type":"app.bsky.embed.images","images":embeds});
        crate::scorecard::validate_reply(record)?;
        Ok(())
    }
    fn upload_blob(&mut self, bytes: &[u8]) -> Result<Value> {
        self.recovered = false;
        self.authenticate()?;
        let mut response = self.upload(bytes)?;
        if matches!(Self::classify(&response), DeliveryError::Auth) {
            self.refresh()?;
            response = self.upload(bytes)?;
        }
        if response.status != 200 {
            return Err(Self::classify(&response).into());
        }
        let blob = &response.value["blob"];
        ensure!(
            blob["size"].as_u64() == Some(bytes.len() as u64),
            "uploaded blob size mismatch"
        );
        Ok(blob.clone())
    }
    fn upload(&self, bytes: &[u8]) -> std::result::Result<Response, DeliveryError> {
        let session = self.session.as_ref().ok_or(DeliveryError::Auth)?;
        let mut response = self
            .agent
            .post(format!(
                "{}/xrpc/com.atproto.repo.uploadBlob",
                session.pds.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {}", session.access_jwt))
            .header("Content-Type", "image/jpeg")
            .send(bytes)
            .map_err(|_| DeliveryError::Retry {
                message: "image upload transport failed".into(),
                after: None,
            })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let bytes = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024)
            .read_to_vec()
            .map_err(|_| DeliveryError::Retry {
                message: "incomplete image upload response".into(),
                after: retry_after,
            })?;
        Ok(Response {
            status,
            value: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            retry_after,
        })
    }
    fn session_path(&self) -> PathBuf {
        self.config.state_dir.join("bluesky-session.json")
    }
    fn request(
        &self,
        pds: &str,
        method: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
        token: Option<&str>,
    ) -> std::result::Result<Response, DeliveryError> {
        let mut url = url::Url::parse(&format!("{}/xrpc/{method}", pds.trim_end_matches('/')))
            .map_err(|_| DeliveryError::Auth)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let response = if body.is_some() || method == "com.atproto.server.refreshSession" {
            let mut req = self.agent.post(url.as_str());
            if let Some(token) = token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            match body {
                Some(body) => req.send_json(body),
                None => req.send_empty(),
            }
        } else {
            let mut req = self.agent.get(url.as_str());
            if let Some(token) = token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            req.call()
        };
        let mut response = response.map_err(|_| DeliveryError::Retry {
            message: "PDS transport result uncertain".into(),
            after: None,
        })?;
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.parse::<i64>().ok().or_else(|| {
                    httpdate::parse_http_date(v)
                        .ok()
                        .and_then(|d| d.duration_since(SystemTime::now()).ok())
                        .map(|d| d.as_secs() as i64)
                })
            });
        let reset = response
            .headers()
            .get("ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok())
            .map(|t| (t - now()).max(0));
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024)
            .read_to_vec()
            .map_err(|_| DeliveryError::Retry {
                message: "incomplete PDS response".into(),
                after: retry_after,
            })?;
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok(Response {
            status,
            value,
            retry_after: retry_after.or(reset),
        })
    }
    fn classify(response: &Response) -> DeliveryError {
        match (response.status, response.value["error"].as_str()) {
            (401 | 403, _)
            | (_, Some("ExpiredToken" | "InvalidToken" | "AuthenticationRequired")) => {
                DeliveryError::Auth
            }
            (_, Some("InvalidSwap")) => DeliveryError::Conflict,
            (429 | 500..=599, _) => DeliveryError::Retry {
                message: format!("PDS HTTP {}", response.status),
                after: response.retry_after,
            },
            (400..=499, _) => DeliveryError::Invalid,
            _ => DeliveryError::Retry {
                message: "unexpected PDS response".into(),
                after: None,
            },
        }
    }
    fn accept_session(
        &mut self,
        value: Value,
        fallback: &str,
    ) -> std::result::Result<(), DeliveryError> {
        if value["did"].as_str() != self.config.bluesky.did.as_deref() {
            return Err(DeliveryError::Auth);
        }
        let pds = Self::pds_from_identity(&value).unwrap_or_else(|| fallback.to_string());
        secure_url(&pds).map_err(|_| DeliveryError::Auth)?;
        let get = |name| {
            value[name]
                .as_str()
                .map(str::to_owned)
                .ok_or(DeliveryError::Auth)
        };
        let session = Session {
            did: get("did")?,
            access_jwt: get("accessJwt")?,
            refresh_jwt: get("refreshJwt")?,
            pds,
        };
        self.save_session(&session)
            .map_err(|_| DeliveryError::Retry {
                message: "cannot persist rotated session".into(),
                after: None,
            })?;
        self.session = Some(session);
        self.verified = true;
        Ok(())
    }
    fn pds_from_identity(value: &Value) -> Option<String> {
        if value["didDoc"]["id"] != value["did"] {
            return None;
        }
        value["didDoc"]["service"].as_array()?.iter().find(|s| {
            s["type"] == "AtprotoPersonalDataServer"
                && s["id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with("#atproto_pds"))
        })?["serviceEndpoint"]
            .as_str()
            .map(str::to_owned)
    }
    fn save_session(&self, session: &Session) -> Result<()> {
        let temporary = self.config.state_dir.join(".session.tmp");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec(session)?)?;
        file.sync_all()?;
        fs::rename(temporary, self.session_path())?;
        fs::File::open(&self.config.state_dir)?.sync_all()?;
        Ok(())
    }
    fn login(&mut self) -> std::result::Result<(), DeliveryError> {
        let password = if let Some(path) = &self.config.bluesky.app_password_file {
            fs::read_to_string(path).map_err(|_| DeliveryError::Auth)?
        } else {
            std::env::var("BLUESKY_APP_PASSWORD").map_err(|_| DeliveryError::Auth)?
        };
        let identifier = self
            .config
            .bluesky
            .identifier
            .as_deref()
            .or(self.config.bluesky.did.as_deref())
            .ok_or(DeliveryError::Auth)?;
        let pds = self.config.bluesky.pds.clone();
        let response = self.request(
            &pds,
            "com.atproto.server.createSession",
            &[],
            Some(&json!({"identifier":identifier,"password":password.trim()})),
            None,
        )?;
        if response.status != 200 {
            return Err(Self::classify(&response));
        }
        self.accept_session(response.value, &pds)
    }
    fn refresh(&mut self) -> std::result::Result<(), DeliveryError> {
        if self.recovered {
            return Err(DeliveryError::Auth);
        }
        self.recovered = true;
        let session = self.session.as_ref().ok_or(DeliveryError::Auth)?;
        let pds = session.pds.clone();
        let response = self.request(
            &pds,
            "com.atproto.server.refreshSession",
            &[],
            None,
            Some(&session.refresh_jwt),
        )?;
        if response.status != 200 {
            return Err(Self::classify(&response));
        }
        self.accept_session(response.value, &pds)
    }
    fn authenticate(&mut self) -> std::result::Result<(), DeliveryError> {
        if self.verified {
            return Ok(());
        }
        if self.session.is_none() && self.session_path().exists() {
            let bytes = fs::read(self.session_path()).map_err(|_| DeliveryError::Auth)?;
            let session: Session =
                serde_json::from_slice(&bytes).map_err(|_| DeliveryError::Auth)?;
            if Some(session.did.as_str()) != self.config.bluesky.did.as_deref() {
                return Err(DeliveryError::Auth);
            }
            secure_url(&session.pds).map_err(|_| DeliveryError::Auth)?;
            self.session = Some(session);
        }
        if let Some(session) = &self.session {
            let result = self.request(
                &session.pds,
                "com.atproto.server.getSession",
                &[],
                None,
                Some(&session.access_jwt),
            )?;
            if result.status == 200 {
                if result.value["did"].as_str() != self.config.bluesky.did.as_deref() {
                    return Err(DeliveryError::Auth);
                }
                if let Some(pds) = Self::pds_from_identity(&result.value) {
                    secure_url(&pds).map_err(|_| DeliveryError::Auth)?;
                    let session = self.session.as_mut().ok_or(DeliveryError::Auth)?;
                    session.pds = pds;
                    self.save_session(self.session.as_ref().ok_or(DeliveryError::Auth)?)
                        .map_err(|_| DeliveryError::Auth)?;
                }
                self.verified = true;
                Ok(())
            } else if matches!(Self::classify(&result), DeliveryError::Auth) {
                self.refresh()
            } else {
                Err(Self::classify(&result))
            }
        } else {
            self.login()
        }
    }
    fn authenticated_request(
        &mut self,
        method: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> std::result::Result<Response, DeliveryError> {
        self.authenticate()?;
        let session = self.session.as_ref().ok_or(DeliveryError::Auth)?;
        let response =
            self.request(&session.pds, method, query, body, Some(&session.access_jwt))?;
        // Authentication recovery is bounded. Subsequent failures pause the adapter.
        if matches!(Self::classify(&response), DeliveryError::Auth) {
            self.refresh()?;
            let session = self.session.as_ref().ok_or(DeliveryError::Auth)?;
            return self.request(&session.pds, method, query, body, Some(&session.access_jwt));
        }
        Ok(response)
    }
    fn receipt(&self, value: &Value, key: &str) -> std::result::Result<Receipt, DeliveryError> {
        let uri = value["uri"].as_str().ok_or_else(|| DeliveryError::Retry {
            message: "missing record URI".into(),
            after: None,
        })?;
        let expected = format!("at://{}/app.bsky.feed.post/{key}", self.account());
        if uri != expected {
            return Err(DeliveryError::Conflict);
        }
        let cid = value["cid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| DeliveryError::Retry {
                message: "missing CID".into(),
                after: None,
            })?;
        Ok(Receipt {
            uri: uri.into(),
            cid: cid.into(),
        })
    }
}
impl Publisher for Bluesky {
    fn platform(&self) -> &'static str {
        "bluesky"
    }
    fn account(&self) -> &str {
        self.config.bluesky.did.as_deref().unwrap_or("")
    }
    fn allocate_identity(&self, previous_clock: i64) -> Result<DeliveryIdentity> {
        next_identity(previous_clock)
    }
    fn template_version(&self) -> i64 {
        render::TEMPLATE_VERSION
    }
    fn prepare(&mut self, observation: &Observation) -> Result<Value> {
        let mut record = render::record(observation, chrono::Utc::now())?;
        let image = media::render(&self.config, observation)?;
        self.attach_image(&mut record, &image)?;
        render::validate(&record).context("validate Bluesky post")?;
        Ok(record)
    }
    fn prepare_permit(&mut self, observation: &Observation, permit: &Permit) -> Result<Value> {
        let mut record = render::permit_record(observation, permit, chrono::Utc::now())?;
        let image = media::render_permit(&self.config, observation, permit)?;
        self.attach_image(&mut record, &image)?;
        render::validate(&record).context("validate permit Bluesky post")?;
        Ok(record)
    }
    fn prepare_permit_reply(
        &mut self,
        observation: &Observation,
        permit: &Permit,
        summary: &WardSummary,
        root_uri: &str,
        root_cid: &str,
    ) -> Result<Value> {
        let mut record =
            render::permit_reply_record(summary, root_uri, root_cid, chrono::Utc::now())?;
        let (near, ward) = maps::render_pair(&self.config, permit, observation)?;
        let mut first = record.clone();
        self.attach_image(&mut first, &near)?;
        let mut second = record.clone();
        self.attach_image(&mut second, &ward)?;
        record["embed"] = json!({"$type":"app.bsky.embed.images","images":[first["embed"]["images"][0].clone(),second["embed"]["images"][0].clone()]});
        render::validate(&record)?;
        Ok(record)
    }
    fn reconcile(
        &mut self,
        key: &str,
        record: &Value,
    ) -> std::result::Result<Reconciliation, DeliveryError> {
        self.recovered = false;
        let account = self.account().to_owned();
        let response = self.authenticated_request(
            "com.atproto.repo.getRecord",
            &[
                ("repo", &account),
                ("collection", "app.bsky.feed.post"),
                ("rkey", key),
            ],
            None,
        )?;
        if response.status == 200 {
            if response.value["value"] == *record {
                return Ok(Reconciliation::Same(self.receipt(&response.value, key)?));
            }
            return Ok(Reconciliation::Conflict);
        }
        if matches!(response.status, 400 | 404) && response.value["error"] == "RecordNotFound" {
            return Ok(Reconciliation::Absent);
        }
        Err(Self::classify(&response))
    }
    fn send(&mut self, key: &str, record: &Value) -> std::result::Result<Receipt, DeliveryError> {
        self.recovered = false;
        let body = json!({"repo":self.account(),"collection":"app.bsky.feed.post","rkey":key,"record":record,"validate":true,"swapRecord":null});
        let response =
            self.authenticated_request("com.atproto.repo.putRecord", &[], Some(&body))?;
        if response.status == 200 {
            self.receipt(&response.value, key)
        } else {
            Err(Self::classify(&response))
        }
    }
}

/// Allocate a monotonic Bluesky TID. The caller persists both values before any send.
pub fn next_identity(previous_clock: i64) -> Result<DeliveryIdentity> {
    let clock = chrono::Utc::now().timestamp_micros().max(
        previous_clock
            .checked_add(1)
            .context("identity clock overflow")?,
    );
    ensure!(
        (0..(1_i64 << 53)).contains(&clock),
        "TID timestamp out of range"
    );
    Ok(DeliveryIdentity {
        key: tid(clock),
        clock,
    })
}
fn tid(timestamp: i64) -> String {
    let mut value = ((timestamp as u64) << 10) | u64::from(rand::random::<u16>() & 1023);
    let alphabet = b"234567abcdefghijklmnopqrstuvwxyz";
    let mut result = [b'2'; 13];
    for c in result.iter_mut().rev() {
        *c = alphabet[(value & 31) as usize];
        value >>= 5;
    }
    result.iter().map(|c| *c as char).collect()
}
