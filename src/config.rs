use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: String,
    pub path: PathBuf,
    pub format: String,
    pub worker_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observer {
    pub host_id: String,
    pub database: PathBuf,
    #[serde(default = "default_poll")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_max")]
    pub max_record_bytes: usize,
}
fn default_poll() -> u64 {
    2000
}
fn default_max() -> usize {
    65536
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub observer: Observer,
    pub sources: Vec<Source>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub host_id: String,
    pub observed_at: String,
    pub sources: Vec<Source>,
}
pub fn resolve(sources: &mut [Source], base: &Path) {
    for source in sources {
        if source.path.is_relative() {
            source.path = base.join(&source.path);
        }
    }
}
pub fn validate(host: &str, sources: &[Source]) -> Result<()> {
    ensure!(!host.trim().is_empty(), "host_id must not be empty");
    ensure!(!sources.is_empty(), "at least one source is required");
    let mut ids = HashSet::new();
    for s in sources {
        ensure!(
            !s.id.is_empty() && ids.insert(&s.id),
            "source IDs must be nonempty and unique"
        );
        ensure!(
            matches!(
                (s.kind.as_str(), s.format.as_str()),
                ("worker_log", "prime95-text-subset-v1") | ("results", "prime95-json-subset-v1")
            ),
            "unsupported kind/format: {}/{}",
            s.kind,
            s.format
        );
        ensure!(
            s.worker_id.as_ref().is_none_or(|id| !id.is_empty()),
            "worker_id must not be empty"
        );
    }
    Ok(())
}
