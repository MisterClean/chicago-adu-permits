use adu_bot::{
    config::Config,
    media,
    publish::{self, bluesky::Bluesky},
    queue, render,
    source::{self, Socrata},
    store::Store,
};
use anyhow::Result;
use clap::{Parser, Subcommand};
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
    Ingest,
    Publish {
        #[arg(long)]
        dry_run: bool,
    },
    Run,
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
    let config = Config::load(cli.config.as_deref())?;
    let mut store = Store::open(&config.state_dir)?;
    match cli.command {
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
