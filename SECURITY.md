# Security policy

## Supported versions

During the initial 0.1 series, security fixes target the latest release and `main`. Older releases may require an upgrade. There is no guaranteed response or fix timeline.

## Report a vulnerability

Use [GitHub's private vulnerability reporting](https://github.com/trivial-si/hs/security/advisories/new). If it is unavailable, email **ivan.filkovic@gmail.com**. Please do not disclose an exploitable vulnerability in a public issue before maintainers have had an opportunity to investigate.

Include the version, operating system, reproduction steps, and impact. Use a minimal synthetic database rather than personal time records, credentials, or client information.

## Local data and agent access

hs stores data in a local SQLite database. Database access is controlled by operating-system permissions; the database is not encrypted by hs. The MCP server uses stdio. Granting an agent access to hs and its database gives that agent access to the exposed operations; preview and revision checks protect against mistakes, not a malicious authorized caller. Backups contain your records and should be protected like the original database.
