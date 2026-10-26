# Kapitel

- Work in this repository. Build and test inside Docker; no host Rust installation is required.
- YAGNI: implement a demonstrated need. Start with the obvious solution.
- Extract shared patterns after the second duplication, not before.
- Delete unused code. Prefer a small number of concrete modules to frameworks and plugin systems.
- Use verbose, semantic names, including units: `playback_position_seconds`.
- Return explicit errors. A bad book must not bring down the library.
- Never write to the library. Store application state separately.
- Keep book identity independent of paths. Never let stale playback overwrite a newer session.
- Never commit real books, credentials, application databases or private host configuration.
- English code, UI and documentation. MIT license.
- Do not claim iPhone validation from a desktop WebKit test. Record real-device checks separately.
