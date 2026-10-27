use crate::{
    config::Source,
    parser::{self, Parsed},
    storage::{Batch, Checkpoint, Record, Store},
};
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
};

pub struct Follower {
    pub source: Source,
    file: Option<File>,
    pub checkpoint: Checkpoint,
}
fn identity(file: &File) -> Result<String> {
    let m = file.metadata()?;
    Ok(format!("{}:{}", m.dev(), m.ino()))
}
fn anchor(file: &mut File, offset: u64) -> Result<String> {
    let start = offset.saturating_sub(128);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (offset - start) as usize];
    file.read_exact(&mut bytes)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
impl Follower {
    pub fn new(store: &Store, host: &str, source: Source) -> Result<Self> {
        let checkpoint = store.register(host, &source)?;
        Ok(Self {
            source,
            file: None,
            checkpoint,
        })
    }
    pub fn poll(
        &mut self,
        store: &mut Store,
        host: &str,
        now: &str,
        max_record: usize,
        inject_failure: bool,
    ) -> Result<usize> {
        ensure!(
            (256..=1_048_576).contains(&max_record),
            "max_record_bytes must be 256..1048576"
        );
        if std::fs::metadata(&self.source.path).is_ok_and(|m| !m.is_file()) {
            store.health(
                &self.source.id,
                "unsupported source: expected a regular file",
                now,
            )?;
            return Ok(0);
        }
        let path_file = File::open(&self.source.path);
        let path_identity = path_file.as_ref().ok().and_then(|f| identity(f).ok());
        if self.file.is_none() {
            match path_file {
                Ok(file) => self.file = Some(file),
                Err(error) => {
                    store.health(&self.source.id, &format!("unavailable: {error}"), now)?;
                    return Ok(0);
                }
            }
        }
        let file = self.file.as_mut().unwrap();
        let current_identity = identity(file)?;
        let mut next = self.checkpoint.clone();
        let length = file.metadata()?.len();
        let changed = next.identity != current_identity
            || length < next.offset
            || (next.offset > 0 && anchor(file, next.offset)? != next.anchor);
        if changed {
            next.generation += 1;
            next.offset = 0;
            next.identity = current_identity.clone();
        }
        let rotating = path_identity
            .as_ref()
            .is_some_and(|i| i != &current_identity);
        file.seek(SeekFrom::Start(next.offset))?;
        let mut bytes = Vec::new();
        // Bounded work and memory per source per poll, even with a growing input.
        file.take((max_record * 2 + 1) as u64)
            .read_to_end(&mut bytes)?;
        let start = next.offset;
        let mut consumed = 0;
        let mut records = Vec::new();
        let mut health = if path_identity.is_none() {
            "path unavailable; following open file"
        } else {
            "ok"
        };
        for (i, b) in bytes.iter().enumerate() {
            if i + 1 - consumed > max_record {
                health =
                    "record_too_large: checkpoint held; fix source or increase max_record_bytes";
                break;
            }
            if *b != b'\n' {
                continue;
            }
            let line = &bytes[consumed..i];
            let raw = String::from_utf8_lossy(line)
                .trim_end_matches('\r')
                .to_string();
            let parsed = if std::str::from_utf8(line).is_err() {
                Parsed::diagnostic("invalid UTF-8 record")
            } else {
                parser::parse(&self.source.format, &raw, self.source.worker_id.as_deref())
            };
            records.push(Record {
                start: start + consumed as u64,
                end: start + i as u64 + 1,
                raw,
                parsed,
            });
            consumed = i + 1;
        }
        next.offset = start + consumed as u64;
        if health == "ok" && consumed < bytes.len() {
            health = "partial_record";
        }
        // Once the old inode is drained, retain its unterminated tail as a diagnostic.
        // Writes to that inode after switching are outside this polling contract.
        if rotating
            && !health.starts_with("record_too_large")
            && length == start + bytes.len() as u64
            && consumed < bytes.len()
        {
            records.push(Record {
                start: next.offset,
                end: length,
                raw: String::from_utf8_lossy(&bytes[consumed..]).into_owned(),
                parsed: Parsed::diagnostic(
                    "unterminated tail on rotated file; retained without state inference",
                ),
            });
            next.offset = length;
            health = "rotation_drained";
        }
        next.anchor = anchor(file, next.offset)?;
        let count = store.commit(
            Batch {
                host,
                source: &self.source,
                previous: &self.checkpoint,
                next: &next,
                records,
                observed_at: now,
                health,
            },
            inject_failure,
        )?;
        self.checkpoint = next;
        if rotating && self.checkpoint.offset == length {
            self.file = None;
        }
        Ok(count)
    }
}
