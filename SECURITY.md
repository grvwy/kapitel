# Security

Report vulnerabilities privately through GitHub's **Report a vulnerability**
facility when enabled. Otherwise request a private channel without posting
exploit details or credentials in a public issue.

Kapitel authenticates metadata/media access, hashes passwords with Argon2id,
uses server-side sessions, checks request origins and CSRF tokens, and confines
media access to the configured library root.

Media is untrusted input. Metadata runs in a resource-limited process with
archive, XML and image limits. These controls do not replace dependency updates.

Run with an unprivileged UID, read-only library/root filesystem, and HTTPS for
remote access. The server administrator and filesystem owner are trusted.
Backups and the data volume contain private account/session information.

v0.1 is a development preview. Fixes target the current release line.

## Dependency review

`cargo audit --deny warnings` gates CI. One narrowly scoped maintenance notice
is accepted in `.cargo/audit.toml`: Lofty's `paste` procedural macro is
unmaintained ([RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html)).
It runs at compile time, and this notice does not report a runtime vulnerability.
Reviewed 2026-10-27 against Lofty 0.25.5; recheck at each release and remove the
exception when upstream replaces it. Security vulnerability advisories are not
suppressed.
