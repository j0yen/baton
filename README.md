# baton

Delegate to the claude session in the *other* window, not to an invisible one. `baton` addresses a visible claude session by its surface — an X11 window-id or a tmux pane — and types a prompt into it, the way you would if you walked over and used the keyboard.

## Why it exists

"Delegate to the other window" has an obvious meaning and a wrong implementation. The wrong one was `delegate.run` v0.1 in `agorabus-worker.sh`: it spawned a fresh headless `claude --print` for every delegated task. The session that "delegated" was invisible — a third claude on the laptop that no one was watching. That is not delegating to a window; it is launching a process and calling it delegation.

`baton` is the addressing primitive that was missing. Every interactive claude registers its surface at SessionStart; the sender resolves the target's surface and types into it via `xdotool` (X11) or `tmux send-keys` (tmux). The delegated work happens in a window you can see and interrupt — which is the whole point of choosing a window over a subprocess.

TIOCSTI is deliberately not used: the kernel locks it on modern Arch (`dev.tty.legacy_tiocsti=0`, confirmed), and an AC enforces that no `src/` path reaches for it.

## Scope

This crate ships the **sender-side CLI** only. The shell components — `baton-register.sh`, `~/.local/lib/baton/injectors/*.sh`, and the `baton.*` method dispatch inside `agorabus-worker.sh` — are separate work, not included here. Built from [PRD-baton.md][prd] (v0.1) via the [autobuilder][ab] skill.

[prd]: https://github.com/j0yen/autobuilder/blob/master/PRD-baton.md
[ab]: https://github.com/j0yen/autobuilder

## Install

```sh
cargo install --path .   # installs the `baton` binary
```

Or build in place:

```sh
cargo build --release
cargo test
```

Targets the Rust 2024 edition, MSRV 1.85, `deny(unsafe_code)`. Dependencies: `clap`, `serde`, `serde_json`, `anyhow`, `dirs`.

## Subcommands

```text
baton peers                                    # list registered surfaces
baton surface <sid>                            # show one surface as JSON
baton send <sid> "<prompt>"                    # type + Enter into the target
baton dry  <sid> "<prompt>"                    # resolve only; type nothing
baton key  <sid> --chord C-c                   # interrupt the target
baton spawn "<prompt>" --target <sid>          # explicit headless (the renamed delegate.run)
```

Global flags: `--from <self-sid>`, `--json`, `--cache-dir <path>`, `--agorabus-bin <path>`. The reply-bearing subcommands take `--deadline-secs`.

## Exit codes

The codes are stable and distinct — a caller can branch on the exact failure rather than parse stderr.

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 2 | `unreachable` — surface gone (target window closed, X11 down) |
| 3 | `cooldown` — receiver rate-limit hit |
| 4 | `not_found` — no cached surface for sid |
| 5 | `timeout` — no reply before `--deadline-secs` |
| 6 | `not_owner` — uid mismatch on the receiver |
| 7 | `replay` — duplicate `(from, id)` rejected by the receiver |
| 8 | `unknown_method` — receiver lacks a `baton.*` handler |
| 9 | Other / publish failure |

## Acceptance criteria

Twelve ACs (7 MUST, 4 SHOULD, 1 MAY) drive the test suite — see `agent/intent-card.json`. Highlights:

- envelope shape matches `AGORABUS_RPC.md` v0.1 (AC3, AC5, AC6)
- ids match `^rpc-baton-[a-z0-9]+$` (AC3)
- a missing `~/.cache/baton/surfaces/` is non-fatal (AC2)
- no TIOCSTI / `setterm` / `/dev/pts/` reference anywhere in `src/` (AC7)
- exit codes are stable and distinct (AC8, AC9, AC11)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
