use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Parsed {
    pub kind: String,
    pub worker_id: Option<String>,
    pub assignment_id: Option<String>,
    pub identity_confidence: String,
    pub exponent: Option<u64>,
    pub iteration: Option<u64>,
    pub total_iterations: Option<u64>,
    pub percent: Option<f64>,
    pub result: Option<String>,
    pub diagnostic: Option<String>,
}
impl Parsed {
    pub fn diagnostic(message: &str) -> Self {
        Self {
            kind: "diagnostic".into(),
            worker_id: None,
            assignment_id: None,
            identity_confidence: "unknown".into(),
            exponent: None,
            iteration: None,
            total_iterations: None,
            percent: None,
            result: None,
            diagnostic: Some(message.into()),
        }
    }
}
static WORKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[Worker #(\d+)[^\]]*\]\s*(.*)$").unwrap());
static START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Starting (?:PRP|primality|Lucas-Lehmer) test of M(\d+)(?:\s|\.|$)").unwrap()
});
static PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Iteration:\s*(\d+)\s*/\s*(\d+)\s*\[([0-9.]+)%\](?:,.*)?$").unwrap()
});
static COMPLETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^M(\d+) (is not prime|is a probable prime|is prime)(?:[.!].*)?$").unwrap()
});

pub fn parse(format: &str, raw: &str, configured_worker: Option<&str>) -> Parsed {
    let mut p = Parsed::diagnostic("unsupported record; no state inferred");
    p.worker_id = configured_worker.map(str::to_owned);
    let line = raw.trim();
    if format == "prime95-json-subset-v1" {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Parsed::diagnostic("invalid JSON result"),
        };
        if value["status"] != "C" {
            return Parsed::diagnostic("unsupported JSON status; expected C (composite result)");
        }
        let Some(exponent) = value["exponent"].as_u64().filter(|n| *n >= 2) else {
            return Parsed::diagnostic("invalid result exponent");
        };
        let Some(worktype) = value["worktype"].as_str().filter(|v| !v.is_empty()) else {
            return Parsed::diagnostic("missing worktype");
        };
        let residue = value["res64"].as_str();
        if residue.is_none_or(|v| v.len() != 16 || !v.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Parsed::diagnostic("expected 16-digit hexadecimal res64");
        }
        p.kind = "completion_observed".into();
        p.exponent = Some(exponent);
        p.result = Some(format!(
            "{worktype}:res64={}",
            residue.unwrap().to_ascii_uppercase()
        ));
        if let Some(aid) = value["aid"]
            .as_str()
            .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            p.assignment_id = Some(format!("aid:{}", aid.to_ascii_lowercase()));
            p.identity_confidence = "explicit_assignment_id".into();
        }
    } else if format == "prime95-text-subset-v1" {
        let captures = WORKER.captures(line);
        let body = if let Some(c) = &captures {
            let worker = format!("worker-{}", &c[1]);
            if configured_worker.is_some_and(|w| w != worker) {
                return Parsed::diagnostic("worker prefix conflicts with configured worker_id");
            }
            p.worker_id = Some(worker);
            &c[2]
        } else {
            line
        };
        if let Some(c) = START.captures(body) {
            p.kind = "assignment_observed".into();
            p.exponent = c[1].parse().ok();
        } else if let Some(c) = PROGRESS.captures(body) {
            p.kind = "progress_observed".into();
            p.iteration = c[1].parse().ok();
            p.total_iterations = c[2].parse().ok();
            p.percent = c[3].parse().ok();
            if p.iteration
                .zip(p.total_iterations)
                .is_none_or(|(i, t)| t == 0 || i > t)
                || p.percent
                    .is_none_or(|n| !n.is_finite() || !(0.0..=100.0).contains(&n))
            {
                return Parsed::diagnostic("invalid progress fields");
            }
        } else if let Some(c) = COMPLETE.captures(body) {
            p.kind = "completion_observed".into();
            p.exponent = c[1].parse().ok();
            p.result = Some(c[2].into());
        } else if body.starts_with("FATAL ERROR:") || body.starts_with("ERROR:") {
            p.kind = "error_observed".into();
            p.diagnostic = Some(body.into());
            return p;
        } else {
            return p;
        }
        if p.kind != "progress_observed" && p.exponent.is_none_or(|n| n < 2) {
            return Parsed::diagnostic("invalid exponent");
        }
    } else {
        return Parsed::diagnostic("unsupported parser format");
    }
    if p.assignment_id.is_none()
        && let Some(exponent) = p.exponent
    {
        p.assignment_id = Some(format!("exponent:{exponent}"));
        p.identity_confidence = "exponent_only_ambiguous".into();
    }
    p.diagnostic = None;
    p
}
