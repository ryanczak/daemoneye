# Contributing to DaemonEye

Thanks for your interest in contributing. DaemonEye is a Linux-only Rust project — please review the requirements below before getting started. Would love to see a, FreeBSD, macOS, or Windows version of DaemonEye!

## Prerequisites

- **Linux** — the project uses `fork(2)`, Unix domain sockets, and Linux-specific tmux hooks. macOS and Windows are not supported.
- **Rust 1.79+** — required by edition 2024. Install via [rustup](https://rustup.rs).
- **tmux 2.6+** — required for hook support. Install via your package manager (`apt install tmux`, `dnf install tmux`, etc.).

## Building

```sh
git clone https://github.com/ryanczak/daemoneye
cd daemoneye
cargo build
```

Release build (optimised binary at `target/release/daemoneye`):

```sh
cargo build --release
```

## Running the Tests

The test suite is self-contained — no running daemon or tmux session required.

```sh
cargo test                    # run all tests (400 tests)
cargo test <name>             # run a single test by name
cargo test -- --nocapture     # show stdout from tests
```

The project must compile with **zero errors and zero new warnings**. Pre-existing `dead_code` warnings are tracked and acceptable; introducing new ones is not.

## Making Changes

### Before you start

Open an issue first for anything beyond a small bug fix or typo. This avoids duplicate effort and gives a chance to discuss approach before you invest time writing code.

### Key architecture files

See `CLAUDE.md` for a full architectural overview and a file-by-file index. A few starting points:

| Area | File(s) |
|---|---|
| IPC wire protocol | `src/ipc.rs` |
| Daemon request handling / AI loop | `src/daemon/server.rs` |
| Tool call dispatch & approval | `src/daemon/executor.rs` |
| AI tool definitions (all backends) | `src/ai/tools.rs`, `src/ai/types.rs` |
| tmux interop | `src/tmux/mod.rs`, `src/tmux/cache.rs` |
| Config & system prompt | `src/config.rs` |

### Adding a new AI tool

Follow the checklist in `CLAUDE.md` under **"Adding a new AI tool"** — it covers all the files that need to change in order.

### Invariants to preserve

- `main()` must stay synchronous so `libc::fork()` is called before the Tokio runtime starts. Do not move the fork inside an async context.
- All mutex lock sites use `.unwrap_or_log()` (the `UnpoisonExt` trait in `src/util.rs`). Do not change these to `.unwrap()`.
- `SRE_PROMPT_TOML` in `src/config.rs` is the canonical system prompt. If you change it, run `cargo test builtin_sre_prompt_parses` to verify it still parses as valid TOML.

## Code Style

Standard Rust conventions apply — `cargo fmt` before committing, `cargo clippy` to catch common issues. No special style guide beyond what the compiler and linter enforce.

```sh
cargo fmt
cargo clippy
```

## Submitting a Pull Request

1. Fork the repository and create a branch from `master`.
2. Make your changes. Add or update tests where appropriate.
3. Verify `cargo test` passes with no new failures.
4. Verify `cargo fmt -- --check` and `cargo clippy` are clean.
5. Open a pull request with a clear description of what changed and why.

## Reporting Issues

Use [GitHub Issues](https://github.com/ryanczak/daemoneye/issues). Include:

- Linux distro and version
- Rust version (`rustc --version`)
- tmux version (`tmux -V`)
- Relevant output from `daemoneye daemon --console` or `~/.daemoneye/var/log/daemon.log`

## License

By contributing you agree that your contributions will be licensed under the [MIT License](LICENSE).
