# Kapitel

Your library. Your place.

A small, self-hosted audiobook player written in Rust. Listen in your browser,
pause, and continue on another device. Your media stays yours: Kapitel only
reads your library and keeps its database in a separate volume.

**v0.1 development preview.** Real iPhone/Safari validation remains a release
gate; desktop WebKit is not a substitute. See [acceptance](docs/acceptance.md).

## Start with Docker

Docker Engine with Compose is the only host prerequisite.

```sh
cp .env.example .env
# Set KAPITEL_LIBRARY_PATH to the absolute host directory containing your books.
docker compose build
docker compose run --rm kapitel user create owner --admin
docker compose up -d
```

The account command prompts without echoing your password. Open
<http://localhost:8080>. The first scan waits for two stable observations,
normally 30 seconds apart. Later scans run every 30 seconds.

The container runs as UID/GID `10001:10001`. Give that identity read/traverse
access to your library, or configure Compose `user` and provision the data
volume for your chosen UID. Your books do not need write permission.

## Library layout

```text
library/
  Audiobooks/
    Book Title/
      01.mp3
      02.mp3
      cover.jpg
      metadata.opf
    Author/Another Book/book.m4b
  Ebooks/A Book/book.epub
```

Use one book per directory. Category and author directories are optional and
can have any name. A standalone media file directly in the root is its own
book. Keep all parts in the same directory; nested disc directories are not
grouped in v0.1. Audio and ebooks in one book directory share a book page.

- Audio: MP3 and AAC in M4A/M4B, including chapter metadata. Original audio
  streams to the browser with byte ranges; there is no transcoding.
- Ebooks: EPUB metadata/covers; PDF/AZW3 identification and local OPF metadata.
  Catalog only: the ebook reader is deferred.
- Metadata: your corrections override OPF, embedded tags, then directory/file
  names. Unknown authors remain unknown.
- Covers: local JPEG/PNG/WebP and embedded artwork become bounded thumbnails.
- Hidden files, symlinks and unfinished `.part` files are ignored. Unsupported
  formats and unreadable files are reported under **Manage**.

Files are identified by SHA-256 content fingerprints. Moving or renaming an
unchanged book preserves its ID, corrections, part order and saved positions.
Exact duplicate copies share one entry. Adding an ebook to an audio folder is
supported. Ambiguous or partial matches require review. Changing embedded tags
or re-encoding changes the fingerprint and is outside automatic recognition.
Missing books remain in the index as unavailable.

## Listening and profiles

Profiles share the library but have individual positions and playback speeds.
Administrators create/disable accounts, reset passwords, edit metadata and
adjust part order. There is no public registration or email dependency.

Play/pause/stop, ±10 seconds, chapter/part navigation, 0.75–2× speed and a
15/30/45/60 minute sleep timer are included. Stop preserves your place;
**Start over** is separate. Playback continues while navigating. Space toggles
playback and arrows skip when focus is outside interactive controls.

**Saved** means the server confirmed the position. Taking over on another
device invalidates the old playback session: delayed writes cannot overwrite
your new position, and deliberate rewinds are preserved. Network failures show
**Not synced** and keep a profile-scoped local recovery point. A newer session
takes precedence when reconnecting. Abrupt shutdown can lose progress since
the last acknowledged save. There is no offline library in v0.1.

Lock-screen controls depend on browser support. Sleep timers are best effort
under background restrictions and require real-device validation before release.

## Remote access

The host binding is loopback only. Put Kapitel behind an HTTPS reverse proxy
or Tailscale Serve and set `KAPITEL_PUBLIC_URL` to its exact origin, for example
`https://books.example.org`. Preserve the browser's `Origin` header and support
GET/HEAD byte-range responses. Do not cache private API/media in a shared cache.

Remote HTTP origins are rejected by default; HTTP is allowed for localhost development.
Secure cookies are automatic for HTTPS. Forwarded client IP headers are not
trusted; sign-in limits are per account and global.

For a trusted LAN test, set `KAPITEL_ALLOW_LAN_HTTP=true` and use an exact
private IP origin such as `http://192.168.1.20:8080`. Explicitly bind the host
port to that LAN address. This opt-in accepts RFC 1918 IPv4 or IPv6 ULA literals,
not public IPs or DNS names. HTTP sends credentials and media without transport
encryption; keep this mode inside your trusted network and use HTTPS remotely.

The Compose example is generic. Host deployment belongs in your infrastructure
repository, not in this project.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `KAPITEL_PUBLIC_URL` | `http://localhost:8080` | Exact browser origin; HTTPS remotely |
| `KAPITEL_ALLOW_LAN_HTTP` | `false` | Explicit private-IP HTTP opt-in for trusted LAN testing |
| `KAPITEL_LIBRARY_DIRECTORY` | `/library` | Read-only path inside the container |
| `KAPITEL_DATA_DIRECTORY` | `/data` | Writable application state |
| `KAPITEL_SCAN_INTERVAL_SECONDS` | `30` | At least 5 seconds |
| `RUST_LOG` | `kapitel=info` | Rust log filter |

`KAPITEL_LIBRARY_PATH` configures the Compose host mount. Configuration errors
prevent startup. Logs go to stdout/stderr. No analytics, external fonts or
external metadata requests are used.

## Backup and recovery

Back up media separately. Stop Kapitel and copy the **complete `/data` volume**,
including SQLite WAL files. Restart after the copy completes. The backup
contains account hashes, sessions and listening history; treat it as private.
Never copy only the main SQLite file while a running database has a WAL.

To restore, stop Kapitel, restore the complete volume with the same owner, and
start the same application version. Check login, books and positions before
upgrading. Schema checks reject incompatible versions. v0.1 creates schema 1;
retain a pre-upgrade backup before future schema migrations.

```sh
docker compose run --rm kapitel user reset owner
```

Password resets invalidate existing sessions. `--password-stdin` is available
for automation. Never put passwords in command arguments or commit them.

## Development

```sh
docker build --target test -t kapitel:test .
docker build --target audit -t kapitel:audit .
sh scripts/test-browser.sh
```

Rust formatting, Clippy and integration contracts run in Docker. Browser tests
run Chromium/WebKit with synthetic media generated inside Docker. Node and
FFmpeg are test-only dependencies. The test stack has its own volumes and does
not mount your real books or database.
The browser script also indexes 1,000 synthetic books, reports API timings and
checks resident memory against 128 MiB. It removes its containers and volumes
on exit; screenshots remain in the ignored `test-results/` directory.

One crate contains accounts, books, indexing, metadata, streaming and playback.
Plain ES modules are embedded in the binary. Metadata extraction uses a
short-lived copy of the binary with memory, CPU, output and wall-time limits.
Application code forbids unsafe Rust; dependencies must still be kept updated.

CI checks pull requests. Main commits publish `sha-<full-commit>` preview images;
version tags publish named releases. Both use generic `linux/amd64` images in
`ghcr.io/grvwy/kapitel` after checks pass. Pin an image digest when deploying.
See [AGENTS.md](AGENTS.md) for project
principles and [SECURITY.md](SECURITY.md) for security reporting. MIT licensed.
