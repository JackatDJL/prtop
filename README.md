# prtop

`prtop` is a keyboard-first, multi-forge PR and MR monitor for a permanent tmux pane.

```sh
cargo run -- --demo
```

The first vertical slice has an asynchronous Ratatui dashboard, deterministic demo data,
normalized forge models, provider boundaries, TOML configuration, and a local cache. Live
GitHub, GitLab, and Forgejo adapters are intentionally limited to the safe read path until
their API fixtures and write confirmations land in later milestones.

## Reviews, comments, and mouse input

Milestone 2 centralizes terminal colors in `ui::theme`. It selects truecolor when available,
then 256-color or ANSI-safe colors. Mouse capture is enabled only while the alternate screen is
active and is disabled during terminal restoration. Clicks and scrolling go through the same
selection and focus state as keyboard navigation.

Comments are stored chronologically. The detail pane begins at the newest ten comments and
scrolls toward older entries. Provider capability flags will gate live write actions before the
UI exposes them.

## Keys

`j`/`k` select, `Enter` toggles detail focus, `/` filters, `r` refreshes, `?` toggles help,
and `q` quits.

## Configuration

On its first normal launch, prtop writes a commented sample to
`~/.config/prtop/config.toml`. See `src/config.rs` for the fully typed shape.

Credentials are resolved into memory only. Environment variables take precedence:

- `PRTOP_GITHUB_TOKEN` or `GITHUB_TOKEN`
- `PRTOP_GITLAB_TOKEN` or `GITLAB_TOKEN`
- `PRTOP_FORGEJO_TOKEN` or `FORGEJO_TOKEN`

Provider adapters may then ask the matching CLI for an access token during discovery. The
CLI is never used as the TUI.
