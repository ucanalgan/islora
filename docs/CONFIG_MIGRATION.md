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

Existing runtime readers keep their legacy fallback until platform orchestration is implemented. In particular, `AlreadyCurrent` classification follows modern schema semantics, while today's unmodified startup reader may still interpret an obsolete density key. A small BOM compatibility fix lets existing readers and writers recognize a first key following the marker without discarding it. Phase 1 does not start migration, modify files, create backups, acquire locks, or change application startup.

Phase 2 owns bounded filesystem reads, locking and Windows file security, protected backup and atomic persistence, post-write verification, startup orchestration, fallback/recovery behavior, and failures. Phase 3 owns any later user-facing migration or recovery workflow. No automatic migration is active yet.
