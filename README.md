# prtop

`prtop` is a keyboard-first, multi-forge PR and MR monitor for a permanent tmux pane.

```sh
cargo run -- --demo
```

## Startup scope

Without an argument, prtop detects the Git repository containing the current directory and opens
that configured project when its remote is recognized. You can also pass `.` or another local
repository path explicitly. Add Cargo's `--` before application arguments:

```sh
cargo run
cargo run -- .
cargo run -- ~/dev/prtop
cargo run -- --global
cargo run -- --demo
```

The installed binary accepts the same application arguments directly:

```sh
prtop
prtop .
prtop ~/dev/prtop
prtop --global
prtop --demo
```

prtop has an asynchronous Ratatui dashboard, deterministic demo data, normalized forge models,
provider boundaries, TOML configuration, and a local cache. GitHub, GitLab, and Forgejo expose
the reads and capability-gated writes supported by their adapters.

## Reviews, comments, and mouse input

Milestone 2 centralizes terminal colors in `ui::theme`. It selects truecolor when available,
then 256-color or ANSI-safe colors. Mouse capture is enabled only while the alternate screen is
active and is disabled during terminal restoration. Clicks and scrolling go through the same
selection and focus state as keyboard navigation.

Comments are stored chronologically. The detail pane begins at the newest ten comments and
scrolls toward older entries. Comments load asynchronously in request detail. Posting is
available where the provider advertises comment support; success reconciles the provider's
comment ID, while failures keep the draft available for retry.

## CI/CD

Focus a change request's CI panel and press `Enter` to open a pipeline, then `Enter` on a job to inspect logs. Pipeline and job screens use `Esc`, `h`, or `q` to move back one level. `R` and `x` ask for confirmation before retry and cancel operations; capabilities and permissions decide which actions are available.

Logs are fetched in the background, retain 30,000 lines by default, support `/` search with `n`/`N`, and use `f` to follow a running job. Older retained content is marked when discarded. GitHub maps workflow runs to pipelines and supports whole-run rerun/cancel. GitLab maps pipelines, stages, jobs, traces, retry/cancel, and manual jobs. Forgejo Actions remains capability-gated because support differs by server version.

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
