# Local validation — 2026-10-27

All builds and tests ran in Docker on Linux amd64, using original synthetic
media. No real library, production database or deployed service was changed.

| Check | Result |
| --- | --- |
| Rust 1.97 formatting and Clippy (`-D warnings`) | Passed |
| Rust unit/integration tests | 11 passed, including explicit LAN HTTP configuration |
| Chromium and WebKit, desktop and 390 × 844 mobile viewport | Passed |
| Initial HTML, CSS and JavaScript | 14,541 bytes gzip; budget 100 KiB |
| Cargo audit, 228 locked packages | Passed with one reviewed maintenance notice; see [security](../SECURITY.md) |
| GitHub Actions workflow / Compose configuration / shell syntax | Passed |
| Release Docker build and complete `scripts/test-browser.sh` | Passed |
| Server resident memory after indexing 1,000 books | 32,420 KiB (31.7 MiB); budget 128 MiB |
| Authenticated 50-book page, 30 local requests | Median 5.25 ms, p95 6.20 ms |
| Runtime image, Linux amd64 | 129,871,950 bytes uncompressed; UID/GID 10001 |

Browser checks cover MP3 and AAC M4B loading, embedded chapters, cross-device
resume, intentional rewind, stale-tab rejection, network recovery, playback
speed, sleep-timer selection, stop, persistent playback during navigation, CSRF
rejection and mobile overflow. Screenshots are generated in `test-results/`.

Integration contracts also cover profile isolation, password revocation,
logout, last-administrator protection, byte ranges, symlink/path escape
rejection, file replacement, moved/restored books, metadata corrections, a
missing library root and malformed files.

The scale check indexed 2 short audiobooks, 6 EPUBs and 992 small, distinct PDF
fixtures from disk. These are local warm API timings, not internet latency or
a benchmark of large audiobook imports. Both browsers passed against the
optimized runtime image. The test script removed its containers and volumes
after success.

## Remaining device gate

Desktop WebKit is not iPhone Safari. Real iPhone/Safari and Windows/Chrome,
locked-screen playback, headset controls, background sleep-timer accuracy and
VoiceOver remain pending in the [acceptance checklist](acceptance.md).
This is a development preview, not a completed production rollout.
