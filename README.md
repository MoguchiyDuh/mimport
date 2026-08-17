# mimport

Personal, single-user music library pipeline. Runs on one VPS, driven mostly by an AI
agent issuing individual CLI commands with `--json` output. Not a service — no auth, no
daemon, no multi-user concerns.

Two rules:

- **Fail loud, no background state.** Every command does one explicit thing and reports
  exactly what happened. No silent retries, continues, or guesses.
- **Semi-automatic, not automatic.** The tool assists decisions (ranking editions,
  proposing matches) but never chains decisions together. A human or agent sits between
  every stage.

The job ends at a correctly tagged, correctly placed file on disk. Navidrome refresh and
listening are out of scope.

## Pipeline

Each stage is a separate, explicit command:

1. **Artist resolution** — search by name, or resolve directly via `mbid:<uuid>`.
2. **Album/release discovery** — list release groups, then the editions within one.
3. **Edition scoring** — rank editions (digital vs CD vs vinyl, studio-bonus vs
   live/remix filler).
4. **Soulseek search** — search slskd for the chosen release.
5. **Fetch** — enqueue downloads by search-id/username/directory; returns a job id.
6. **Status** — non-blocking poll of a job's transfer state.
7. **Postfix** — strip junk tags, downsample if needed.
8. **Import** — match files to the release's tracks, write clean tags (incl. cover art),
   copy into the library.
9. **Library** — browse/query/remove over the index `import` populates.
10. **YT fetch** — separate URL-only path for YouTube/YouTube Music sources: yt-dlp
    fetches Opus audio, tags are set manually (or backfilled from one `--release`
    track), then it lands in the library same as `import`.

## Commands

```
mimport lidarr artist <query>                       # text or mbid:<uuid>
mimport lidarr album <release-group-mbid>           # ranked editions
mimport lidarr tracks <release-group-mbid> <release-mbid>

mimport mb artist <query>
mimport mb album <release-group-mbid>
mimport mb tracks <release-mbid>
mimport mb track <text>

mimport slskd search <query>
mimport slskd search-status <id>
mimport slskd fetch <search-id> <username> <directory> [<filename>] [--title <text>]
mimport slskd status <job-id|title>
mimport slskd cancel <job-id|title> [--remove]
mimport slskd browse <username> [<directory>]

mimport postfix <job-id|path|title> [--dry-run]
mimport import <job-id|path|title> --release <mbid> [--force <mapping.json>] [--dry-run]

mimport library list [<query>...]
mimport library show <id>
mimport library remove [--files] [<query>...]

mimport yt fetch <url> [--title <text>] [--artist <text>] [--album <text>]
                        [--track <n>] [--disc <n>] [--year <text>]
                        [--release <mbid> --track <n>] [--dry-run]
```

Every command supports `--json`. `library` query terms support `field:value`, `~fuzzy`,
`lo..hi` ranges, and `-`/`^` negation (see `mimport library list` behaviour).

## Config

Copy `config.example.toml` to `config.toml`:

- `[paths]` — library, downloads, staging, database.
- `[musicbrainz]` — UA (required), rate limit, cache.
- `[lidarr]` — `api.lidarr.audio` proxy (cache only).
- `[slskd]` — Soulseek daemon URL, username/password, fetch timeouts.
- `[quality]` — postfix downsample target.
- `[scoring]` — edition-scorer weights (optional, sane defaults).
- `[cover_art]` — Cover Art Archive base URL (optional).
- `[yt]` — `yt_dlp_path` (optional, defaults to `yt-dlp` on `$PATH`).

## Build

Rust 2024 (stable). Workspace: `mimport-core` (lib) + `mimport` (bin).

```
cargo build --release
```

## Non-goals

No `mimport pull <artist>` one-shot, no autoimport/autofetch, no daemon/scheduler, no
web UI, no plugin system. Not a lidarr/beets clone — deliberately narrower scope.
