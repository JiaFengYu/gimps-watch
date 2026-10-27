use anyhow::{Result, bail, ensure};
use clap::{Parser, Subcommand};
use gimps_watch::{
    config::{self, Config, Manifest},
    reader::Follower,
    storage::Store,
};
use std::{net::SocketAddr, path::PathBuf, time::Duration};
#[derive(Parser)]
#[command(
    version,
    about = "Read-only local observer for a documented Prime95 log subset"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Replay {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        db: PathBuf,
    },
    Watch {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        once: bool,
    },
    Status {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Events {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        cursor: i64,
    },
    Serve {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: SocketAddr,
    },
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Replay { manifest, db } => {
            let mut m: Manifest = serde_json::from_str(&std::fs::read_to_string(&manifest)?)?;
            chrono::DateTime::parse_from_rfc3339(&m.observed_at)?;
            config::resolve(&mut m.sources, manifest.parent().unwrap());
            config::validate(&m.host_id, &m.sources)?;
            let mut store = Store::open(&db)?;
            let mut total = 0;
            for source in m.sources {
                let mut follower = Follower::new(&store, &m.host_id, source)?;
                loop {
                    let n = follower.poll(&mut store, &m.host_id, &m.observed_at, 65536, false)?;
                    total += n;
                    if n == 0 {
                        break;
                    }
                }
            }
            let sources = store.sources()?;
            for source in sources.as_array().unwrap() {
                ensure!(
                    source["health"] == "ok",
                    "replay incomplete for {}: {}",
                    source["id"],
                    source["health"]
                );
            }
            println!(
                "{}",
                serde_json::json!({"ingested_records":total,"status":store.status()?})
            );
        }
        Command::Watch { config: path, once } => {
            let mut c: Config = toml::from_str(&std::fs::read_to_string(&path)?)?;
            let base = path.parent().unwrap();
            config::resolve(&mut c.sources, base);
            if c.observer.database.is_relative() {
                c.observer.database = base.join(&c.observer.database);
            }
            config::validate(&c.observer.host_id, &c.sources)?;
            ensure!(
                (100..=60000).contains(&c.observer.poll_interval_ms),
                "poll interval must be 100..60000 ms"
            );
            let mut store = Store::open(&c.observer.database)?;
            let mut followers = c
                .sources
                .into_iter()
                .map(|s| Follower::new(&store, &c.observer.host_id, s))
                .collect::<Result<Vec<_>>>()?;
            let mut timer =
                tokio::time::interval(Duration::from_millis(c.observer.poll_interval_ms));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = tokio::signal::ctrl_c() => break, _ = timer.tick() => {} }
                for follower in &mut followers {
                    follower.poll(
                        &mut store,
                        &c.observer.host_id,
                        &chrono::Utc::now().to_rfc3339(),
                        c.observer.max_record_bytes,
                        false,
                    )?;
                }
                if once {
                    break;
                }
            }
        }
        Command::Status { db, json } => {
            let value = Store::read_only(&db)?.status()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("GIMPS observations\n{}\n", value["note"].as_str().unwrap());
                for worker in value["workers"].as_array().unwrap() {
                    println!(
                        "{}  {}  assignment={}  progress={}  last={}",
                        worker["worker_id"],
                        worker["state"],
                        worker["assignment_id"],
                        worker["percent"],
                        worker["last_observed_at"]
                    );
                }
                for source in value["sources"].as_array().unwrap() {
                    println!(
                        "source {}: {} (offset {})",
                        source["id"], source["health"], source["offset"]
                    );
                }
            }
        }
        Command::Events { db, limit, cursor } => println!(
            "{}",
            serde_json::to_string_pretty(&Store::read_only(&db)?.events(limit, cursor)?)?
        ),
        Command::Serve { db, listen } => {
            if !listen.ip().is_loopback() {
                bail!("MVP API is local-only; use a loopback address and SSH forwarding");
            }
            Store::read_only(&db)?.status()?;
            let listener = tokio::net::TcpListener::bind(listen).await?;
            eprintln!("GIMPS dashboard: http://{}", listener.local_addr()?);
            axum::serve(listener, gimps_watch::api::router(db))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
    }
    Ok(())
}
