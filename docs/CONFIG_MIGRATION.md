# Configuration migration core: Step 3, Phase 1

`island_core::migration::prepare(&str)` is pure: it examines UTF-8 text and returns `Result<MigrationOutcome, MigrationError>`. `Prepared { text }` contains one complete version 2 document; `AlreadyCurrent` means keep the input exactly as supplied; `Future { version }` means preserve the newer document and use read-only compatibility. An error means no migration has been prepared. Error values identify a category and, where applicable, a critical key or line number; they do not include source lines or configuration values.

Schema versions are independent of application versions. Missing `config_version` or `"1"` is legacy; `"2"` is current. Positive versions above 2 are future. Empty, non-decimal, zero, or overflowing versions are invalid. Version 2 documents are not normalized, even if they contain `ui_density`. This core has no byte entry point: callers must reject invalid UTF-8 before constructing `&str`.

| Legacy `ui_density` | Indicator | Media |
| --- | --- | --- |
| `minimal` | `ring` | `compact` |
| `balanced` | `edge` | `standard` |
| `detailed` | `context` | `informative` |

Each valid modern value wins independently. A missing or invalid modern value is derived from a valid legacy value. With no legacy value, absent modern values keep their existing defaults (`edge` and `standard`); an invalid explicit modern value is rejected. An invalid legacy value is rejected when either modern value needs it, but can be removed when both modern values are valid. Successful preparation removes `ui_density` and writes `config_version = "2"` in the same output. The resulting presentation is checked against `Settings::read`.

Duplicate active `config_version`, `ui_density`, `indicator_style`, or `media_layout` assignments are errors, even if identical. Commented lines do not count. Critical values accept the existing simple quoted or unquoted syntax and whitespace-delimited inline comments; malformed quoting, unexpected delimiters, or ambiguous critical lines are rejected. Non-critical assignments, including their values, are not decoded or normalized, and unrelated duplicate keys are retained. Empty and comment-only files are valid legacy input; unsupported bare lines and section headers are rejected rather than guessed. This is a deliberately narrow line-oriented format, not a full TOML parser.

Existing unrelated lines, comments, sensitive values, ordering, UTF-8 BOM, and individual LF/CRLF endings are preserved. A removed legacy line's inline comment becomes a standalone comment. Replaced critical assignments retain their inline comments. New keys are appended in indicator, media, version order, using the predominant line ending (LF on a tie or with no line endings). The source's final-newline status is preserved; when an unterminated final legacy line is removed, the newly exposed final separator is removed if necessary. Input and output are each limited to 1,048,576 bytes; oversized output fails instead of returning a truncated document.

The application presentation reader permits legacy fallback only when the core accepts the legacy input; current and future schemas ignore `ui_density`, and invalid input uses presentation defaults. This is a targeted read-time safeguard, not a verified bootstrap or a runtime recovery mode. Other settings still use the existing readers. A small BOM compatibility fix lets existing readers and writers recognize a first key following the marker without discarding it. Phase 1 does not start migration, modify files, create backups, acquire locks, or change application startup.

Phase 2 owns bounded filesystem reads, locking and Windows file security, protected backup and atomic persistence, post-write verification, startup orchestration, fallback/recovery behavior, and failures. Phase 3 owns any later user-facing migration or recovery workflow. No automatic migration is active yet.

## Phase 2 work in progress

The existing application settings read checks the opened file size and limits the number of bytes read to 1 MiB plus one, then requires valid UTF-8. This applies to the existing settings reader and callers including the calendar. A failed read does not normalize or truncate the active file. Settings writes receive the underlying read error rather than a generic read failure. The ordinary writer refuses a source classified as future or invalid, and refuses output beyond 1 MiB. This check has a time-of-check/time-of-use race because writes are not coordinated with migration or external editors; it is not a complete read-only mode.

Fresh files now include `config_version = "2"`. If the primary is missing, recognised draft, recovery, or protected-backup artifacts prevent it being mistaken for a fresh install. A protected backup or interrupted recovery draft also blocks automatic selection of a rolling backup. The existing rolling-backup path rejects oversized, invalid UTF-8, invalid-schema and future-schema backups before copying, and compares the copied content before restoring. A valid active primary still takes precedence. These checks leave ambiguous artifacts intact.

The active file and rolling backup also receive a conservative path-based reparse-point check. This check is **not** handle-based: another process can replace a path between checking and opening it, ancestor junctions are not validated, and ACLs are not checked or preserved explicitly. Existing draft creation, copying, and replacement retain similar race and permission risks. The rolling-backup path therefore is not yet the secure migration recovery required for Phase 2; users should not assume it has those guarantees.

This is **not** complete Phase 2 migration. No runtime migration, shared writer lock, protected backup creation, explicit schema-specific runtime mode, security-descriptor verification, controlled retry, or live recovery has been enabled. Those guarantees must be implemented and tested before automatic migration is activated.
