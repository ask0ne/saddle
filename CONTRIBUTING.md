# Contributing to Saddle

Thanks for your interest in Saddle. This project is in beta, and issues, ideas and pull requests are all welcome.

## Scope

Saddle is a **macOS-only** app, and it targets **Terminal.app** only. It reads what Claude Code already writes to disk and raises the right Terminal tab. Support for other terminals, other platforms or other tools is out of scope for now. If you want to work on one of those, please open an issue first so we can talk it through.

## Development setup

You need:

- macOS 13 or later
- [Rust](https://rustup.rs) (stable)
- [Node.js](https://nodejs.org) 22 or later
- `jq` (used by `install-hooks.sh`)
- Claude Code, to see real sessions

```bash
git clone https://github.com/ask0ne/saddle.git
cd saddle/app
npm install
npm run tauri dev
```

`npm run tauri dev` hot-reloads the frontend on save. The first Rust build takes about a minute; later ones take seconds.

To build a release bundle:

```bash
cd app && npm run tauri build
```

## Checks

Run these before opening a pull request. CI runs the same ones.

```bash
# Rust, from app/src-tauri
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo deny check            # licences, advisories, sources (cargo install cargo-deny)

# Frontend, from app
npm run check

# From the repository root
scripts/test-install-hooks.sh   # hook installer
scripts/check-version.sh        # version consistency across files
```

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org): `feat(app): ...`, `fix(app): ...`, `docs: ...`, `chore: ...`.
- Keep pull requests small and focused on one change.
- Branch from `main` and open the pull request against `main`.
- Describe what changed and why, and how you tested it. Add or update tests for behavior changes.
- The minimum supported Rust version is `rust-version` in `app/src-tauri/Cargo.toml`; CI builds with exactly that.
- The app version lives in `app/src-tauri/Cargo.toml` and `app/package.json` (kept equal by `scripts/check-version.sh`). `tauri.conf.json` intentionally has no `version`.
- `main` is protected: changes land through a pull request, and commits must be signed.
- Update `CHANGELOG.md` under `Unreleased` for user-visible changes.

## The `~/.devpit` path

Saddle was previously called devpit. The hook event log is still written to `~/.devpit/events.jsonl`, and the installer's marker is still `# devpit`. This is intentional: hooks already installed on a machine keep working, and renaming the path would silently break them until the installer is run again. Please do not rename it as part of an unrelated change.

## Reporting bugs and security issues

Use the issue templates for bugs and feature requests. For security issues, follow [SECURITY.md](SECURITY.md) and do not open a public issue.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
