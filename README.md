# baton

Cross-window claude delegation primitive: address a *visible* claude session
in another terminal and type a prompt into it, rather than spawning an
invisible headless instance.

Built from [PRD-baton.md][prd] (v0.1) via the [autobuilder][ab] skill.
This crate ships the **sender-side CLI** only — the shell components
(`baton-register.sh`, `~/.local/lib/baton/injectors/*.sh`, and the
`baton.*` method dispatch inside `agorabus-worker.sh`) are separate work.

[prd]: https://github.com/j0yen/autobuilder/blob/master/PRD-baton.md
[ab]: https://github.com/j0yen/autobuilder

## Why

`delegate.run` v0.1 (in `agorabus-worker.sh`) spawned a fresh headless
`claude --print` for every delegated task. The session that "delegated"
was invisible — a third claude on the laptop that no one was watching.
That isn't what "delegate to the other window" means.

`baton` is the missing addressing primitive: every interactive claude
registers its surface (X11 window-id or tmux pane) at SessionStart; the
sender resolves the target's surface and types into it via `xdotool` or
`tmux send-keys`. TIOCSTI is intentionally not used (kernel locks it on
modern Arch — `dev.tty.legacy_tiocsti=0` confirmed).

## Subcommands

```text
baton peers                                    # list registered surfaces
baton surface <sid>                            # show one surface as JSON
baton send <sid> "<prompt>"                    # type + Enter into target
baton dry  <sid> "<prompt>"                    # resolve only; type nothing
baton key  <sid> --chord C-c                   # interrupt the target
baton spawn "<prompt>" --target <sid>          # explicit headless (renamed delegate.run)
```

Global flags: `--from <self-sid>`, `--json`, `--cache-dir <path>`,
`--agorabus-bin <path>`.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 2 | `unreachable` — surface gone (target window closed, X11 down) |
| 3 | `cooldown` — receiver rate-limit hit |
| 4 | `not_found` — no cached surface for sid |
| 5 | `timeout` — no reply before `--deadline-secs` |
| 6 | `not_owner` — uid mismatch on receiver |
| 7 | `replay` — duplicate `(from, id)` rejected by receiver |
| 8 | `unknown_method` — receiver lacks `baton.*` handler |
| 9 | Other / publish failure |

## Acceptance criteria

12 ACs (7 MUST, 4 SHOULD, 1 MAY) drive the test suite — see
`agent/intent-card.json`. Highlights:

- envelope shape matches AGORABUS_RPC.md v0.1 (AC3, AC5, AC6)
- ids match `^rpc-baton-[a-z0-9]+$` (AC3)
- `~/.cache/baton/surfaces/` is non-fatal when missing (AC2)
- no TIOCSTI / `setterm` / `/dev/pts/` references in `src/` (AC7)
- exit codes are stable and distinct (AC8, AC9, AC11)

## Build

```sh
cargo build --release
cargo test
```

Targets Rust 2024 edition, MSRV 1.85, `deny(unsafe_code)`. Deps:
`clap`, `serde`, `serde_json`, `anyhow`, `dirs`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option.
