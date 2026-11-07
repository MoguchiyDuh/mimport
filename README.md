# mimport

A single-user CLI for curating a local music library. It resolves artists and
releases through MusicBrainz (directly or via the Lidarr metadata proxy), ranks
editions, fetches releases from Soulseek through [slskd] or from YouTube through
[yt-dlp], and imports correctly tagged, correctly named files into the library.

The job ends at a tagged file in the right place on disk. Serving and playback
(Navidrome, etc.) are out of scope.

## Design

- **One explicit step per command.** Search, fetch, status, and import are
  separate invocations. Nothing chains decisions, retries silently, or runs in
  the background — a human or a script sits between every stage.
- **Fail loud.** Ambiguous matches, missing tracks, and unromanized non-Latin
  fields block the import unless explicitly overridden.
- **Machine-readable.** Every command accepts `--json`.

Output is always JSON: pretty-printed by default, compact with `--json`. Errors
are written to stderr and return a nonzero exit status.

## Requirements

- Python 3.13+ and [uv](https://docs.astral.sh/uv/)
- `ffmpeg` on `$PATH` — `libsoxr` for downsampling, `libopus` for Opus conversion
- [slskd] ≥ 0.26 for Soulseek, with
  `transfers.download.destination.subdirectory` set to
  `${SOURCE_USERNAME}/${SOURCE_DIRECTORY}` so the on-disk layout matches
  `<downloads>/<username>/<remote dir>`
- [yt-dlp] for `yt fetch` (optional)

## Install

```sh
git clone https://github.com/MoguchiyDuh/mimport.git
cd mimport
uv run mimport --help
```

Run commands from the checkout with `uv run mimport`. uv manages the environment
and dependencies. No Rust toolchain, compilation, updater, or release binary is
needed. Dependencies use prebuilt wheels.

Zsh completion:

```sh
source <(uv run mimport completion zsh)
```

## Configuration

All configuration lives in one TOML file, `config.toml` in the working
directory by default (`--config <path>` to override). There are no environment
variables. Start from the example:

```
cp config.example.toml config.toml
```

`config.toml` holds the slskd credentials and is gitignored.

| Section         | Purpose |
| --------------- | ------- |
| `[paths]`       | `library`, `downloads`, `staging`, `database` |
| `[musicbrainz]` | `user_agent` (required, include a contact), rate limit, disk cache, retries |
| `[lidarr]`      | Lidarr metadata proxy base URL and disk cache |
| `[slskd]`       | Daemon URL, `username`/`password`, request and fetch timeouts |
| `[quality]`     | What import does to lossless files (see below) |
| `[cover_art]`   | Cover Art Archive URL, cache, iTunes fallback |
| `[yt]`          | `yt_dlp_path`, optional `cookies` / `cookies_from_browser` |
| `[scoring]`     | Optional edition-scorer weights |

**Quality.** `codec = "flac"` (default) resamples lossless files above
`target_samplerate`/`target_bitdepth` (44100/16 by default). `codec = "opus"`
transcodes every lossless file to Opus at `opus_bitrate_kbps` (default 160,
VBR, 48 kHz). Lossy files are never re-encoded. Originals are removed only with
`--move` or `--cleanup`, after a successful import.

**Fetch timeout.** The wait window is
`fetch_timeout_base_secs + fetch_timeout_per_mb_secs × MB + fetch_timeout_per_file_secs × files`,
re-armed whenever progress is observed. `slskd fetch --wait-secs` overrides it.

**Cover art.** Fetching is opt-in (`import --cover-art`, `cover --fetch`).
When the Cover Art Archive has nothing for a release, the iTunes Search API is
tried (`itunes_fallback`, `itunes_country`).

## Workflow

```
mimport lidarr artist "<name>"                       # artist mbid
mimport lidarr album <release-group-mbid>            # ranked editions -> release mbid
mimport slskd search "<artist> <album>"              # lossless-only top matches
mimport slskd fetch <search-id> <user> <dir>         # returns a job id
mimport slskd status <job-id>
mimport import <job-id> --release <release-mbid> --cover-art --cleanup
```

Prefer `lidarr` over `mb` for discovery and tracklists: the proxy answers in a
single call, while `mb` talks to MusicBrainz directly at ~1 req/s with several
round trips per lookup. `mb` covers what the proxy lacks (`mb track` recording
search).

Import does, in order:

1. Resolve release metadata and validate manual overrides.
2. Prepare temporary copies, strip junk tags, and apply the `[quality]` policy.
3. Match files to the release's tracks (Hungarian assignment over title,
   duration, artist, track number, and recording MBID); unmatched files block
   the import. `--force <mapping.json>` bypasses matching.
4. Write a fresh tag set, including the release group's top-voted MusicBrainz
   genres.
5. Publish into
   `<library>/<Artist>/<Album> (<Year>)/<NN> - <Title>.<ext>`, with a
   `<DD>-` disc prefix on multi-disc releases.
6. Commit library records, then delete sources only with `--move` or `--cleanup`.

Original files remain untouched during preparation and matching. Publication
uses temporary files, collision checks, and rollback on handled failures.
Interrupted operations retain a `.mimport-operation-*.json` recovery manifest
under the library root; filesystem changes and SQLite commits are not one atomic
transaction. Cover art is embedded before publication when requested.

The command examples below omit the `uv run` prefix for readability.

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
                                    [--label <n>] [--album-artist <n>] [--compilation]
                                    [--genre <n>]...
                                    [--track-title <[disc:]pos>=<title>]...
                                    [--cover <img> | --cover-art]
                                    [--allow-native] [--move] [--allow-partial]
                                    [--cleanup] [--dry-run]

mimport library list [<query>...]
mimport library show <id>
mimport library edit <id> [--rename] [--dry-run] <field=value>...
mimport library remove [--files] [<query>...]
mimport library prune [--dry-run]
mimport cover [<query>...] [--fetch]

mimport yt fetch <url> [--title <t>] [--artist <a>] [--album <a>]
                      [--track <n>] [--disc <n>] [--year <y>]
                      [--album-artist <a>] [--compilation] [--genre <g>]...
                      [--release <mbid> --track <n>] [--playlist]
                      [--tags <overrides.json>] [--allow-native]
                      [--cookies <jar>] [--cookies-from-browser <name>]
                      [--dry-run]

mimport completion zsh
```

`mimport <command> --help` documents every flag.

### Soulseek search

`slskd search` and `search-status` show a lossless-only view by default,
ranked by free upload slot, then upload speed, then queue length, and trimmed
to the top responses. `--all` shows everything. Fetch selectors always resolve
against the full, unfiltered search.

### Library queries

`library` and `cover` take query terms as trailing arguments, one shell token
per clause:

| Term          | Meaning |
| ------------- | ------- |
| `field:value` | match a field |
| `~text`       | fuzzy match |
| `lo..hi`      | numeric range |
| `-term`, `^term` | negation |

Flags such as `--files` must come before the terms.

`library edit` takes `field=value` pairs — `title`, `artist`, `album_artist`,
`album`, `year`, `track`, `disc`, `compilation` (`1`/`0`/`true`/`false`), and
repeatable `genre` — rewrites the file's tags, and with `--rename` re-derives
the library path. `album_artist`, `compilation`, and `genre` are tag-only.
Editing `artist` also updates `ALBUMARTIST`/`ARTISTS` unless `album_artist` is
set in the same call.

### Tag overrides

`import` and `yt fetch` accept `--tags <file.json>`:

```json
{
  "artist": "...",
  "album": "...",
  "album_artist": "Various Artists",
  "compilation": true,
  "date": "2024-01-01",
  "label": "...",
  "genres": ["..."],
  "cover": "/path/to/cover.jpg",
  "tracks": { "1": "Title", "2:3": "Disc 2 track 3" }
}
```

Individual flags win over the file for the same field; `genre` and `genres`
are merged. `--album-artist` and `--compilation` let Various-Artists
compilations group as one album.

Non-Latin (CJK/Hangul) fields are romanized via MusicBrainz aliases, preferring
Latin-script romaji. Without a usable alias or a manual override the import
fails unless `--allow-native` is passed.

### YouTube

`yt fetch` takes a video or playlist URL (`--playlist`), downloads Opus audio
with yt-dlp, and tags it either from flags/`--tags` or by backfilling from a
MusicBrainz release (`--release <mbid> --track <n>`). The source URL is stored
in a `SOURCEURL` tag and indexed in the library.

`--dry-run` obtains metadata only: no media or thumbnails are downloaded, no tags
are written, and the library and database are unchanged.

[slskd]: https://github.com/slskd/slskd
[yt-dlp]: https://github.com/yt-dlp/yt-dlp
