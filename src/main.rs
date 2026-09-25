use adu_bot::{
    config::Config,
    media,
    publish::{self, bluesky::Bluesky, scorecards},
    queue, render,
    source::{self, Socrata},
    store::Store,
};
use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Monitor Chicago ADU preapprovals; building permits are a separate process"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Schema version supported by this release, without opening state.
    SchemaVersion,
    /// Inspect schema and integrity without changing the database or calling external services.
    Check {
        /// Fail on stale ingestion, paused publishing, or deliveries needing attention.
        #[arg(long)]
        health: bool,
    },
    /// Upgrade an existing database; caller must exclude normal runs with deployment.lock.
    Migrate,
    Ingest,
    Publish {
        #[arg(long)]
        dry_run: bool,
    },
    Run,
    Scorecards {
        #[command(subcommand)]
        action: ScorecardCommand,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Preview {
        #[arg(long)]
        application_id: String,
        /// Write a full-resolution JPEG and sibling .alt.txt. Fetches Street View; does not post.
        #[arg(long)]
        image: Option<PathBuf>,
    },
    Queue {
        #[command(subcommand)]
        action: QueueCommand,
    },
    Backup {
        destination: PathBuf,
    },
    Adapter {
        #[command(subcommand)]
        action: AdapterCommand,
    },
}
#[derive(Subcommand)]
enum QueueCommand {
    List,
    Inspect {
        event_key: String,
    },
    Approve {
        event_key: String,
        #[arg(long)]
        reason: String,
    },
    Suppress {
        event_key: String,
        #[arg(long)]
        reason: String,
    },
    Retry {
        event_key: String,
        #[arg(long)]
        reason: String,
    },
}
#[derive(Subcommand)]
enum AdapterCommand {
    /// Check app-password authentication without publishing.
    Check,
    Resume {
        #[arg(long)]
        reason: String,
    },
}
#[derive(Subcommand)]
enum ScorecardCommand {
    /// Prepare and publish one due scorecard reply.
    Run,
    /// Send a queued reply using locally rendered, source-verified maps.
    RunPrepared {
        id: i64,
        #[arg(long)]
        input_dir: PathBuf,
    },
    /// Render the source-backed reply locally without authenticating or posting.
    Preview {
        application_id: String,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Queue a reviewed historical announcement; does not publish.
    Enqueue {
        application_id: String,
        #[arg(long)]
        reason: String,
    },
    List,
    Inspect {
        id: i64,
    },
    /// Reconcile a held or failed reply using its original identity.
    Retry {
        id: i64,
        #[arg(long)]
        reason: String,
    },
}
fn main() {
    if let Err(error) = run() {
        eprintln!(
            "{}",
            serde_json::json!({"level":"error","message":format!("{error:#}")})
        );
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Command::SchemaVersion) {
        println!("{}", adu_bot::migrations::CURRENT);
        return Ok(());
    }
    let config = Config::load(cli.config.as_deref())?;
    if let Command::Check { health } = cli.command {
        return check(&config, health);
    }
    if matches!(cli.command, Command::Migrate) {
        ensure!(
            config.state_dir.join("adu.sqlite3").is_file(),
            "database missing"
        );
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(config.state_dir.join("writer.lock"))?;
        FileExt::try_lock_exclusive(&lock)?;
        let mut db = rusqlite::Connection::open_with_flags(
            config.state_dir.join("adu.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        adu_bot::migrations::apply(&mut db)?;
        return check(&config, false);
    }
    ensure!(
        !config.require_existing_state || config.state_dir.join("adu.sqlite3").is_file(),
        "required production database is missing; refusing to create a new baseline"
    );
    std::fs::create_dir_all(&config.state_dir)?;
    let deployment_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config.state_dir.join("deployment.lock"))?;
    FileExt::lock_shared(&deployment_lock)?;
    ensure!(
        !config.state_dir.join("deployment-blocked").exists()
            || !matches!(
                cli.command,
                Command::Run
                    | Command::Publish { dry_run: false }
                    | Command::Ingest
                    | Command::Scorecards {
                        action: ScorecardCommand::Run
                    }
                    | Command::Scorecards {
                        action: ScorecardCommand::RunPrepared { .. }
                    }
            ),
        "deployment recovery required; runtime is blocked"
    );
    let mut store = Store::open(&config.state_dir)?;
    match cli.command {
        Command::SchemaVersion | Command::Check { .. } | Command::Migrate => unreachable!(),
        Command::Ingest => source::ingest(&mut store, &mut Socrata::new(&config), &config)?,
        Command::Run => {
            let started = std::time::Instant::now();
            let ingestion = if store.due_ingest(config.ingest_interval_seconds)? {
                source::ingest(&mut store, &mut Socrata::new(&config), &config)
            } else {
                Ok(())
            };
            let mut remaining = config.clone();
            remaining.max_run_seconds = config
                .max_run_seconds
                .saturating_sub(started.elapsed().as_secs());
            let publishing = if config.publish_enabled {
                publish::publish(&mut store, &remaining, &mut Bluesky::new(&config)?)
            } else {
                Ok(())
            };
            ingestion?;
            publishing?;
        }
        Command::Scorecards { action } => match action {
            ScorecardCommand::Run => {
                scorecards::run(&mut store, &config, &mut Bluesky::new(&config)?)?
            }
            ScorecardCommand::RunPrepared { id, input_dir } => scorecards::run_prepared(
                &mut store,
                &config,
                &mut Bluesky::new(&config)?,
                id,
                &input_dir,
            )?,
            ScorecardCommand::Preview {
                application_id,
                output_dir,
            } => scorecards::preview(&store, &config, &application_id, &output_dir)?,
            ScorecardCommand::Enqueue {
                application_id,
                reason,
            } => println!(
                "{}",
                scorecards::enqueue(&store, &config, &application_id, &reason)?
            ),
            ScorecardCommand::List => println!(
                "{}",
                serde_json::to_string_pretty(&scorecards::list(&store)?)?
            ),
            ScorecardCommand::Inspect { id } => println!(
                "{}",
                serde_json::to_string_pretty(&scorecards::inspect(&store, id)?)?
            ),
            ScorecardCommand::Retry { id, reason } => scorecards::retry(&store, id, &reason)?,
        },
        Command::Publish { dry_run: true } => publish::dry_run(&store)?,
        Command::Publish { dry_run: false } => {
            publish::publish(&mut store, &config, &mut Bluesky::new(&config)?)?
        }
        Command::Status { json } => {
            let status = store.status()?;
            if json {
                println!("{}", serde_json::to_string(&status)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&status)?);
            }
        }
        Command::Preview {
            application_id,
            image,
        } => {
            let observation = store.application(&application_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&render::record(&observation, chrono::Utc::now())?)?
            );
            if let Some(path) = image {
                let card = media::render(&config, &observation)?;
                std::fs::write(&path, &card.bytes)?;
                std::fs::write(path.with_extension("alt.txt"), &card.alt)?;
                eprintln!(
                    "Card: {} ({} × {}, {} bytes, JPEG quality {})",
                    path.display(),
                    media::WIDTH,
                    media::HEIGHT,
                    card.bytes.len(),
                    card.quality
                );
            }
        }
        Command::Backup { destination } => store.backup(&destination)?,
        Command::Queue { action } => match action {
            QueueCommand::List => {
                println!("{}", serde_json::to_string_pretty(&queue::list(&store)?)?)
            }
            QueueCommand::Inspect { event_key } => println!(
                "{}",
                serde_json::to_string_pretty(&queue::inspect(&store, &event_key)?)?
            ),
            QueueCommand::Approve { event_key, reason } => {
                queue::review(&mut store, &event_key, "approve", &reason)?
            }
            QueueCommand::Suppress { event_key, reason } => {
                queue::review(&mut store, &event_key, "suppress", &reason)?
            }
            QueueCommand::Retry { event_key, reason } => {
                queue::review(&mut store, &event_key, "retry", &reason)?
            }
        },
        Command::Adapter {
            action: AdapterCommand::Check,
        } => {
            println!("{}", Bluesky::new(&config)?.check_auth()?);
        }
        Command::Adapter {
            action: AdapterCommand::Resume { reason },
        } => publish::resume(&store, &config, &reason)?,
    }
    Ok(())
}

fn check(config: &Config, health: bool) -> Result<()> {
    let db = rusqlite::Connection::open_with_flags(
        config.state_dir.join("adu.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        version == adu_bot::migrations::CURRENT,
        "incompatible schema {version}"
    );
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(integrity == "ok", "database integrity check failed");
    ensure!(
        db.prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_none(),
        "foreign key check failed"
    );
    let baseline: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM source_state)", [], |r| {
        r.get(0)
    })?;
    ensure!(
        !config.require_existing_state || baseline,
        "required baseline missing"
    );
    let last: Option<i64> = db.query_row(
        "SELECT max(ended_at) FROM ingest_runs WHERE status='success'",
        [],
        |r| r.get(0),
    )?;
    let attention: i64 = db.query_row(
        "SELECT (SELECT count(*) FROM deliveries WHERE state IN ('held','failed'))+(SELECT count(*) FROM reply_deliveries WHERE state IN ('held','failed'))",
        [],
        |r| r.get(0),
    )?;
    let oldest_reply:Option<i64>=db.query_row("SELECT min(enqueued_at) FROM reply_deliveries WHERE state IN ('pending','prepared','sending','retry')",[],|r|r.get(0))?;
    let paused: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM source_state WHERE posting_paused=1) OR EXISTS(SELECT 1 FROM adapter_state WHERE paused=1)", [], |r| r.get(0))?;
    println!(
        "{}",
        serde_json::json!({"schema":version,"integrity":integrity,"baseline_established":baseline,"last_success":last,"attention_deliveries":attention,"oldest_scorecard_queue_age_seconds":oldest_reply.map(|at|(adu_bot::store::now()-at).max(0)),"paused":paused})
    );
    if health {
        ensure!(
            last.is_some_and(|at| adu_bot::store::now() - at <= config.stale_after_seconds),
            "source ingestion is stale"
        );
        ensure!(
            !paused && config.publish_enabled,
            "publishing is paused or disabled"
        );
        ensure!(attention == 0, "deliveries require review");
        if config.scorecards.enabled {
            ensure!(
                oldest_reply.is_none_or(|at| adu_bot::store::now() - at <= 86400),
                "scorecard reply queue is older than 24 hours"
            );
        }
    }
    Ok(())
}
