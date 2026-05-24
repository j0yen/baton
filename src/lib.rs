//! baton — sender-side library for cross-window claude delegation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_SETTLE_MS: u32 = 750;
pub const DEFAULT_DEADLINE_SECS: u64 = 30;

/// AGORABUS RPC envelope shape (v0.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    pub id: String,
    pub from: String,
    pub to: String,
    pub method: String,
    pub params: serde_json::Value,
    pub deadline_unix: u64,
}

impl Envelope {
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Generate an id of shape `rpc-baton-<10 base36 chars>`.
pub fn make_id(seed: u128) -> String {
    let mut s = seed;
    let mut out = String::with_capacity(10);
    for _ in 0..10 {
        let d_u32 = u32::try_from(s % 36).unwrap_or(0);
        let d = u8::try_from(d_u32).unwrap_or(0);
        let c = if d < 10 {
            char::from(b'0' + d)
        } else {
            char::from(b'a' + (d - 10))
        };
        out.push(c);
        s /= 36;
    }
    format!("rpc-baton-{out}")
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

pub fn build_send_envelope(
    from: &str,
    to: &str,
    prompt: &str,
    id: String,
    deadline_unix: u64,
    dry_run: bool,
    submit: bool,
    settle_ms: u32,
) -> Envelope {
    let params = serde_json::json!({
        "prompt": prompt,
        "target_session_id": to,
        "dry_run": dry_run,
        "submit": submit,
        "settle_ms": settle_ms,
    });
    Envelope {
        id,
        from: from.to_string(),
        to: to.to_string(),
        method: "baton.send".to_string(),
        params,
        deadline_unix,
    }
}

pub fn build_key_envelope(
    from: &str,
    to: &str,
    chord: &str,
    repeat: u32,
    id: String,
    deadline_unix: u64,
) -> Envelope {
    let params = serde_json::json!({
        "chord": chord,
        "target_session_id": to,
        "repeat": repeat,
    });
    Envelope {
        id,
        from: from.to_string(),
        to: to.to_string(),
        method: "baton.key".to_string(),
        params,
        deadline_unix,
    }
}

pub fn build_spawn_envelope(
    from: &str,
    to: &str,
    prompt: &str,
    cwd: Option<&str>,
    id: String,
    deadline_unix: u64,
) -> Envelope {
    let params = if let Some(c) = cwd {
        serde_json::json!({"prompt": prompt, "cwd": c})
    } else {
        serde_json::json!({"prompt": prompt})
    };
    Envelope {
        id,
        from: from.to_string(),
        to: to.to_string(),
        method: "baton.spawn".to_string(),
        params,
        deadline_unix,
    }
}

/// Registered surface descriptor as published on `baton.surface.<sid>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SurfaceRecord {
    pub session_id: String,
    pub surface: serde_json::Value,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    #[serde(default)]
    pub claude_version: String,
    #[serde(default)]
    pub registered_unix: u64,
}

pub fn default_surface_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("baton").join("surfaces"))
}

pub fn load_surface_cache(dir: &Path) -> Vec<SurfaceRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(rec) = serde_json::from_slice::<SurfaceRecord>(&bytes) {
            out.push(rec);
        }
    }
    out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    out
}

pub fn load_surface_by_sid(dir: &Path, sid: &str) -> Option<SurfaceRecord> {
    let path = dir.join(format!("{sid}.json"));
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<SurfaceRecord>(&bytes).ok()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReplyEnvelope {
    pub id: String,
    #[serde(default)]
    pub from: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyOutcome {
    Ok,
    Unreachable,
    Cooldown,
    NotOwner,
    Replay,
    UnknownMethod,
    Timeout,
    NotFound,
    OtherError,
}

pub fn classify_reply(reply: &ReplyEnvelope) -> ReplyOutcome {
    if reply.ok {
        return ReplyOutcome::Ok;
    }
    match reply.error.as_deref() {
        Some("unreachable") => ReplyOutcome::Unreachable,
        Some("cooldown") => ReplyOutcome::Cooldown,
        Some("not_owner") => ReplyOutcome::NotOwner,
        Some("replay") => ReplyOutcome::Replay,
        Some("unknown_method") => ReplyOutcome::UnknownMethod,
        Some("not_found") => ReplyOutcome::NotFound,
        _ => ReplyOutcome::OtherError,
    }
}

pub fn outcome_exit_code(outcome: ReplyOutcome) -> u8 {
    match outcome {
        ReplyOutcome::Ok => 0,
        ReplyOutcome::Unreachable => 2,
        ReplyOutcome::Cooldown => 3,
        ReplyOutcome::NotFound => 4,
        ReplyOutcome::Timeout => 5,
        ReplyOutcome::NotOwner => 6,
        ReplyOutcome::Replay => 7,
        ReplyOutcome::UnknownMethod => 8,
        ReplyOutcome::OtherError => 9,
    }
}

// AC7-SCAN-EXEMPT-START
//
// The terms listed below are the definition of what AC7 forbids.
// The acceptance_ac7 test recognizes this exemption marker and skips
// lines between START and END so this function does not match itself.
/// Static-grep terms that must NOT appear in this codebase (AC7).
pub fn forbidden_terms() -> &'static [&'static str] {
    &["TIOCSTI", "/dev/pts/", "setterm"]
}
// AC7-SCAN-EXEMPT-END

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_shape_matches_pattern() {
        let id = make_id(42);
        assert!(id.starts_with("rpc-baton-"), "got: {id}");
        assert_eq!(id.len(), "rpc-baton-".len() + 10);
        assert!(
            id.chars()
                .skip("rpc-baton-".len())
                .all(|c| c.is_ascii_alphanumeric())
        );
    }
}
