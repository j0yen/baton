//! baton — CLI driver for cross-window claude delegation.

use anyhow::{Context, Result, bail};
use baton::{
    DEFAULT_DEADLINE_SECS, DEFAULT_SETTLE_MS, Envelope, ReplyEnvelope, build_key_envelope,
    build_send_envelope, build_spawn_envelope, classify_reply, default_surface_cache_dir,
    load_surface_by_sid, load_surface_cache, make_id, now_unix, now_unix_nanos,
    outcome_exit_code,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

#[derive(Parser, Debug)]
#[command(name = "baton", version, about = "Cross-window claude delegation primitive")]
struct Cli {
    #[arg(long, global = true)]
    from: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,
    #[arg(long, global = true, default_value = "agorabus")]
    agorabus_bin: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List registered surfaces from the cache directory.
    Peers,
    /// Show a single registered surface by session-id.
    Surface { sid: String },
    /// Type a prompt + Enter into the target session's surface.
    Send {
        sid: String,
        prompt: String,
        #[arg(long, default_value_t = DEFAULT_DEADLINE_SECS)]
        deadline_secs: u64,
        #[arg(long, default_value_t = true)]
        submit: bool,
        #[arg(long, default_value_t = DEFAULT_SETTLE_MS)]
        settle_ms: u32,
        #[arg(long)]
        no_publish: bool,
    },
    /// Resolve a surface for the target but type nothing.
    Dry {
        sid: String,
        prompt: String,
        #[arg(long, default_value_t = DEFAULT_DEADLINE_SECS)]
        deadline_secs: u64,
        #[arg(long)]
        no_publish: bool,
    },
    /// Inject a single keychord into the target.
    Key {
        sid: String,
        #[arg(long)]
        chord: String,
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        #[arg(long, default_value_t = DEFAULT_DEADLINE_SECS)]
        deadline_secs: u64,
        #[arg(long)]
        no_publish: bool,
    },
    /// Explicit fire-and-forget headless (renamed delegate.run shim).
    Spawn {
        prompt: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, default_value_t = 300)]
        deadline_secs: u64,
        #[arg(long)]
        no_publish: bool,
    },
}

fn self_sid(opt: Option<String>) -> String {
    opt.or_else(|| std::env::var("AGORABUS_SESSION_ID").ok())
        .unwrap_or_else(|| format!("baton-cli-{}", std::process::id()))
}

fn cache_dir(opt: Option<PathBuf>) -> Option<PathBuf> {
    opt.or_else(default_surface_cache_dir)
}

fn cmd_peers(cache: Option<PathBuf>, json: bool) -> Result<u8> {
    let Some(dir) = cache else {
        if json {
            println!("[]");
        } else {
            eprintln!("baton: no cache directory; nothing to list");
        }
        return Ok(0);
    };
    let records = load_surface_cache(&dir);
    if json {
        let s = serde_json::to_string(&records).context("serialize peers")?;
        println!("{s}");
    } else if records.is_empty() {
        eprintln!("baton: no registered surfaces in {}", dir.display());
    } else {
        for r in &records {
            let kind = r
                .surface
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!(
                "{}  {}  registered={}",
                r.session_id, kind, r.registered_unix
            );
        }
    }
    Ok(0)
}

fn cmd_surface(cache: Option<PathBuf>, sid: &str, json: bool) -> Result<u8> {
    let Some(dir) = cache else {
        eprintln!("baton: no cache directory");
        return Ok(4);
    };
    match load_surface_by_sid(&dir, sid) {
        Some(rec) => {
            let s = if json {
                serde_json::to_string(&rec).context("serialize surface")?
            } else {
                serde_json::to_string_pretty(&rec).context("serialize surface")?
            };
            println!("{s}");
            Ok(0)
        }
        None => {
            eprintln!("baton: not_found sid={sid}");
            Ok(4)
        }
    }
}

fn publish_envelope(bin: &str, sid: &str, topic: &str, env: &Envelope) -> Result<()> {
    let payload = env.to_json_string().context("serialize envelope")?;
    let output = Command::new(bin)
        .args(["publish", "--session-id", sid, topic])
        .arg(&payload)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("invoke {bin} publish"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("agorabus publish failed: {stderr}");
    }
    Ok(())
}

fn await_reply(
    bin: &str,
    self_sid: &str,
    env_id: &str,
    deadline_unix: u64,
) -> Result<Option<ReplyEnvelope>> {
    let topic = format!("rpc.reply.{self_sid}");
    let listener_sid = format!("{self_sid}-baton-reply-{}", std::process::id());
    let now = now_unix();
    if deadline_unix <= now {
        return Ok(None);
    }
    let timeout_secs = deadline_unix - now;
    let mut child = Command::new("timeout")
        .arg(format!("{timeout_secs}s"))
        .arg(bin)
        .args([
            "subscribe",
            &topic,
            "--session-id",
            &listener_sid,
            "--max-events",
            "16",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {bin} subscribe"))?;
    let stdout = child.stdout.take().context("no stdout from subscribe")?;
    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(stdout);
    let mut matched: Option<ReplyEnvelope> = None;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let data = value.get("data").cloned().unwrap_or(serde_json::Value::Null);
        let Ok(reply) = serde_json::from_value::<ReplyEnvelope>(data) else {
            continue;
        };
        if reply.id == env_id {
            matched = Some(reply);
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(matched)
}

fn run_send_like(
    bin: &str,
    self_sid: &str,
    env: Envelope,
    no_publish: bool,
    json: bool,
) -> Result<u8> {
    if no_publish || std::env::var("BATON_NO_PUBLISH").is_ok() {
        let s = env.to_json_string().context("serialize envelope")?;
        println!("{s}");
        return Ok(0);
    }
    let target_topic = format!("rpc.req.{}", env.to);
    let env_id = env.id.clone();
    let deadline = env.deadline_unix;
    publish_envelope(bin, self_sid, &target_topic, &env)?;
    let reply = await_reply(bin, self_sid, &env_id, deadline)?;
    let Some(reply) = reply else {
        eprintln!("baton: timeout waiting for reply to {env_id}");
        return Ok(outcome_exit_code(baton::ReplyOutcome::Timeout));
    };
    let outcome = classify_reply(&reply);
    let code = outcome_exit_code(outcome);
    if json {
        let s = serde_json::to_string(&serde_json::json!({
            "id": reply.id,
            "ok": reply.ok,
            "error": reply.error,
            "detail": reply.detail,
            "result": reply.result,
        }))
        .context("serialize reply")?;
        println!("{s}");
    } else if !reply.ok {
        let err = reply.error.as_deref().unwrap_or("error");
        let detail = reply.detail.as_deref().unwrap_or("");
        eprintln!("baton: {err} {detail}");
    } else if let Some(r) = &reply.result {
        let s = serde_json::to_string(r).unwrap_or_default();
        println!("{s}");
    }
    Ok(code)
}

fn try_main() -> Result<u8> {
    let cli = Cli::parse();
    let self_sid_str = self_sid(cli.from.clone());
    let cache = cache_dir(cli.cache_dir.clone());

    match cli.cmd {
        Cmd::Peers => cmd_peers(cache, cli.json),
        Cmd::Surface { sid } => cmd_surface(cache, &sid, cli.json),
        Cmd::Send {
            sid,
            prompt,
            deadline_secs,
            submit,
            settle_ms,
            no_publish,
        } => {
            let id = make_id(now_unix_nanos());
            let deadline = now_unix() + deadline_secs;
            let env = build_send_envelope(
                &self_sid_str,
                &sid,
                &prompt,
                id,
                deadline,
                false,
                submit,
                settle_ms,
            );
            run_send_like(&cli.agorabus_bin, &self_sid_str, env, no_publish, cli.json)
        }
        Cmd::Dry {
            sid,
            prompt,
            deadline_secs,
            no_publish,
        } => {
            let id = make_id(now_unix_nanos());
            let deadline = now_unix() + deadline_secs;
            let env = build_send_envelope(
                &self_sid_str,
                &sid,
                &prompt,
                id,
                deadline,
                true,
                false,
                DEFAULT_SETTLE_MS,
            );
            run_send_like(&cli.agorabus_bin, &self_sid_str, env, no_publish, cli.json)
        }
        Cmd::Key {
            sid,
            chord,
            repeat,
            deadline_secs,
            no_publish,
        } => {
            let id = make_id(now_unix_nanos());
            let deadline = now_unix() + deadline_secs;
            let env = build_key_envelope(&self_sid_str, &sid, &chord, repeat, id, deadline);
            run_send_like(&cli.agorabus_bin, &self_sid_str, env, no_publish, cli.json)
        }
        Cmd::Spawn {
            prompt,
            target,
            cwd,
            deadline_secs,
            no_publish,
        } => {
            let target = target.unwrap_or_else(|| self_sid_str.clone());
            let id = make_id(now_unix_nanos());
            let deadline = now_unix() + deadline_secs;
            let env = build_spawn_envelope(
                &self_sid_str,
                &target,
                &prompt,
                cwd.as_deref(),
                id,
                deadline,
            );
            run_send_like(&cli.agorabus_bin, &self_sid_str, env, no_publish, cli.json)
        }
    }
}

fn main() -> ExitCode {
    match try_main() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("baton: {e:#}");
            ExitCode::from(9)
        }
    }
}
