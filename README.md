# mimport

A single-user CLI pipeline for curating a music library. Resolve artists and albums via
MusicBrainz/Lidarr, score and pick the best edition, fetch the release from Soulseek (or
YouTube), post-process tags, and import correctly-tagged files into the library.

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
7. **Import** — automatically postfixes first (strips junk tags, downsamples
   lossless to the `[quality]` target — 16/44 by default), then matches files
   to the release's tracks, writes clean tags, and copies (or `--move`) into
   the library. Cover art is opt-in: `--cover <img>` embeds a local image,
   `--cover-art` fetches the front cover from the Cover Art Archive (cached
   on disk, iTunes fallback). `--cleanup` deletes the imported source files
   afterwards, so downloads don't pile up on disk.
8. **Library** — browse/query/edit/remove over the index `import` populates,
   plus a `cover` command to backfill cover art into already-imported tracks.
9. **YT fetch** — separate URL-only path for YouTube/YouTube Music sources: yt-dlp
    fetches Opus audio (single video or a whole playlist), tags are set manually or
    backfilled from a MusicBrainz release, then it lands in the library same as
    `import`.

## Agent notes

mimport is designed to be driven by an AI agent issuing CLI commands with
`--json`. Two conventions keep that fast and correct:

- **Prefer the `lidarr` commands** for discovery, edition ranking, and
  tracklists — the proxy is one fast call. `mb` hits MusicBrainz directly at
  ~1 req/s with multiple round trips per lookup, and every `mb` command warns
  on stderr for exactly this reason. Use `mb` only for what the proxy lacks
  (`mb track` recording search) or when the proxy is unavailable.
- **Job ids over paths**: `slskd fetch` returns a job id; `import` and the
  job-scoped `slskd` commands take `<job-id|title|path>` — pass the id
  through, don't reconstruct paths.

Typical album flow:

```
mimport lidarr album <release-group-mbid>          # pick an edition's release mbid
mimport slskd search "<artist> album"              # lossless-only top matches
mimport slskd fetch <search-id> <user> <dir>       # returns job id
mimport import <job-id> --release <release-mbid> --cover-art --cleanup
```

## Commands

```
mimport lidarr artist <query>                       # text or mbid:<uuid>
mimport lidarr album <release-group-mbid>           # ranked editions
mimport lidarr tracks <release-group-mbid> <release-mbid>

mimport mb artist <query>
mimport mb album <release-group-mbid>
mimport mb tracks <release-mbid>
mimport mb track <text>

mimport slskd search <query> [--fresh] [--all]
mimport slskd searches
mimport slskd search-status <id> [--all]
mimport slskd search-remove <id>
mimport slskd fetch <search-id> <username> <directory> [<filename>...]
                      [--title <text>] [--wait-secs <n>]
mimport slskd status <job-id|title>
mimport slskd cancel <job-id|title> [--remove]
mimport slskd retry <job-id|title> [--wait-secs <n>]
mimport slskd downloads
mimport slskd remove <username> <transfer-id>
mimport slskd clear-completed
mimport slskd browse <username> [<directory>]

mimport import <job-id|path|title> --release <mbid> [--force <mapping.json>]
                                    [--tags <overrides.json>]
                                    [--artist <n>] [--album <n>] [--date <d>]
                                    [--label <n>] [--genre <n>]
                                    [--track-title <[disc:]pos>=<title>]...
                                    [--cover <img> | --cover-art]
                                    [--allow-native] [--move] [--allow-partial]
                                    [--cleanup] [--dry-run]

mimport library list [<query>...]
mimport library show <id>
mimport library edit <id> [--rename] [--dry-run] <field=value>...
mimport library remove [--files] [<query>...]
mimport cover [<query>...] [--fetch]

mimport yt fetch <url> [--title <t>] [--artist <a>] [--album <a>]
                      [--track <n>] [--disc <n>] [--year <y>]
                      [--release <mbid> --track <n>] [--playlist]
                      [--tags <overrides.json>] [--allow-native]
                      [--cookies <jar>] [--cookies-from-browser <name>]
                      [--dry-run]
```

Every command supports `--json`. `library`/`cover` query terms support
`field:value`, `~fuzzy`, `lo..hi` ranges, and `-`/`^` negation.

`slskd search`/`search-status` present a lossless-only view by default,
ranked free-slot first, then fastest upload speed, then shortest queue —
top few responses only; `--all` shows everything. Fetch selectors always
resolve against the full, unfiltered search.

`library edit` takes `field=value` pairs (`title`, `artist`, `album`,
`year`, `track`, `disc`), rewrites the file's tags to match, and with
`--rename` re-derives the full library path from the naming scheme.

Import/yt tag overrides: `--tags` takes a JSON file (`artist`, `album`, `date`,
`label`, `genre`, `cover`, per-position `tracks`); the individual flags win over
the file for the same field. Non-Latin (CJK/Hangul) fields are romanized via
MusicBrainz aliases; without a usable alias or manual override the import fails
unless `--allow-native` is passed (on yt, `--tags`/`--allow-native` require
`--release`).

## Config

All configuration lives in a single `config.toml` — no `.env`, no environment
variables. Copy `config.example.toml` to `config.toml` and fill it in. `config.toml`
is gitignored because it holds the credentials below; keep it out of version control.

- `[paths]` — library, downloads, staging, database.
- `[musicbrainz]` — UA (required), rate limit, disk cache, retries.
- `[lidarr]` — `api.lidarr.audio` proxy (cache only).
- `[slskd]` — Soulseek daemon URL, and **username/password** (sensitive). Fetch
  wait window = `fetch_timeout_base_secs + fetch_timeout_per_mb_secs × total MB
  + fetch_timeout_per_file_secs × file count`, re-armed on any observed
  progress; `fetch --wait-secs` overrides it per call. Requires slskd ≥ 0.26
  with `transfers.download.destination.subdirectory` set to
  `${SOURCE_USERNAME}/${SOURCE_DIRECTORY}` so on-disk layout matches the job's
  `local_dir` (`<downloads>/<username>/<remote parent dirname>`).
- `[quality]` — import's automatic downsample target (lossless files above
  this rate/depth are resampled during import; lossy files are never touched).
- `[scoring]` — edition-scorer weights (optional, sane defaults).
- `[cover_art]` — Cover Art Archive base URL and disk cache dir (optional; fetch is opt-in
  via `import --cover-art` / `cover --fetch`). `itunes_fallback` (default on) falls back
  to the iTunes Search API (`itunes_country`, default `JP`) when the archive has no
  cover for a release.
- `[yt]` — `yt_dlp_path` (optional, defaults to `yt-dlp` on `$PATH`), plus optional
  `cookies` (Netscape-format jar) / `cookies_from_browser` passed through to yt-dlp;
  the `yt fetch --cookies*` flags override these per call.

Sensitive values (`slskd.username`, `slskd.password`) live in `config.toml` alongside
everything else — there is no separate secrets file.

## Build

Requires Rust (stable, edition 2024).

```
cargo build --release
```
