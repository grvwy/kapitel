# v0.1 acceptance

Automated and real-device results must be recorded separately. Desktop WebKit
does not establish iPhone background behavior.

## Automated gates

- Docker Rust tests, strict Clippy and formatting pass.
- Session takeover, rewinds, duplicate and reordered writes preserve position.
- Profiles cannot read/overwrite each other's positions.
- Authentication protects metadata, covers and GET/HEAD/Range media.
- CSRF/origin checks, password reset and last-admin protection pass.
- Streaming returns exact ranges and rejects changed files.
- Moves preserve identity and corrections. A missing root preserves the index.
- Bad files do not block unrelated books; metadata limits are enforced.
- Chromium/WebKit playback, navigation, resume and mobile layout checks pass.
- Initial HTML/CSS/JS stays below 100 KiB gzip, excluding covers.
- Idle memory target: below 128 MiB with 1,000 synthetic books.
- Dependency audit has no unreviewed vulnerabilities.

See [validation results](validation.md) for the latest local run and its limits.

## Real iPhone/Safari and Windows/Chrome — pending

Record device, OS, browser version and date:

- [ ] Chrome pause + Saved → Safari resume within one second.
- [ ] Safari rewind + Saved → Chrome resumes at the rewound position.
- [ ] Old Chrome tab cannot overwrite Safari after takeover.
- [ ] Locked iPhone, headset and lock-screen controls work.
- [ ] Cross an MP3 part boundary while locked.
- [ ] Chapters and seeks work in a long M4B.
- [ ] Sleep timer while locked: record actual stop-time deviation.
- [ ] Network interruption, pause and reconnection report correct sync state.
- [ ] Force-close Safari: resume at the last acknowledged save.
- [ ] Rename/move a book and verify identity and position after scanning.
- [ ] Keyboard navigation, VoiceOver and 200% zoom remain usable.

The release remains a preview until these pass. No real-device results have
been claimed by the implementation agent.

## Homelab rollout

Use a separate infrastructure PR with a pinned image, the existing library
read-only and a new state volume. Keep Audiobookshelf available during
evaluation. Deployment goes through the existing workflow, never direct server
changes. Audiobookshelf accounts and progress are not migrated in v0.1.
