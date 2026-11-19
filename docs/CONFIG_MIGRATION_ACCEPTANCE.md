# Step 3 acceptance audit

**Gate: BLOCKED.** Phase 1 (`66e504a`) provides pure migration preparation. Phase 2 (`4da27e5`) added bounded UTF-8 reads. Subsequent uncommitted work adds limited write validation, version-scoped application presentation reading, current-schema fresh files, and conservative artifact checks. These do not integrate migration, constitute a complete runtime read-only mode, or eliminate races. No production migration runs at startup.

Evidence abbreviations: **M** = `crates/island-core/src/migration.rs` and its `tests`; **S** = `crates/island-app/src/platform/settings.rs` and its tests; **A** = `crates/island-app/src/app.rs`; **C** = `.github/workflows/ci.yml`. **Unit** refers to automated pure/core tests; **FS** to temporary-directory filesystem tests. Static observations are not proof of Windows security or live behavior. Status is **Partial**, **Missing**, **Core verified** (only pure logic), or **Configured** (CI definition only). No row marked Core verified establishes end-to-end migration.

| # | Decision | Status and evidence | Remaining risk |
|---|---|---|---|
| 1 | Automatic Safe Migration | Missing — M is not called from startup | No automatic migration. |
| 2 | Protected Migration Backup | Missing — S uses only rolling `.bak` | No protected source copy. |
| 3 | Safe Fallback | Missing — S/A have no migration outcomes | Invalid files do not enter a verified fallback. |
| 4 | Remove `ui_density` after migration | Partial — M `legacy_values_map_to_the_existing_feature_preferences` (Unit) | No commit to disk. |
| 5 | Configuration Schema Version | Partial — M version classification (Unit); S `fresh_configuration_is_created_with_current_schema` (FS) | Existing legacy files are not migrated. |
| 6 | Future Config Read-Only Compatibility | Partial — M classification; S `settings_writes_reject_future_and_invalid_sources_without_creating_drafts`, `unsupported_lines_do_not_block_ordinary_writes` (FS) and `write_permission_refuses_only_unsafe_sources`, `unsupported_lines_do_not_hide_unsafe_settings` (Unit); A `default_autostart_is_skipped_when_the_configuration_cannot_be_written`, `presentation_classification_ignores_unsupported_lines` (Unit) | No runtime mode or write coordination; TOCTOU remains. |
| 7 | Preserve Explicit Preferences | Partial — M `explicit_modern_preferences_are_independent_and_win` (Unit) | No persisted migration verification. |
| 8 | Atomic Migration | Missing — no migration writer | Existing S `replace` is not a verified migration commit. |
| 9 | Silent Success, Visible Failure | Partial — S reports normal write failures; A skips the first-start autostart default without a dialog when the file cannot be written (Unit) | No migration-specific outcome or UI; the skipped default is not surfaced. |
| 10 | Version-Gated Migration | Partial — M `prepare` (Unit) | No startup gate. |
| 11 | Fresh Install Bypass | Partial — S `fresh_configuration_is_created_with_current_schema` and `missing_primary_with_recovery_evidence_does_not_create_defaults` (FS) | Artifact checks and creation are not coordinated or handle-secured. |
| 12 | Controlled Retry | Missing — no retry state | Invalid source can be reconsidered on every read. |
| 13 | Protected Backup Retention | Missing — no protected backup implementation | Retention not applicable. |
| 14 | Comprehensive Migration Test Suite | Partial — M tests and S ordinary-write tests | No Phase 2 end-to-end/security/concurrency suite. |
| 15 | Pre-Initialization Migration | Missing — A `run` | No verified bootstrap before initialization. |
| 16 | Minimal-Diff Migration | Core verified — M `unrelated_values_comments_and_sensitive_data_survive` (Unit) | No on-disk migration. |
| 17 | Strict Conflict Detection | Core verified — M `duplicate_critical_keys_are_conflicts_even_when_values_match` (Unit) | App guard races with external edits. |
| 18 | Strict Legacy Validation | Core verified — M `legacy_and_modern_invalid_values_are_not_guessed` (Unit) | No validated runtime fallback. |
| 19 | Validated Legacy Detection | Partial — M classification (Unit) | App lacks safe fallback/bootstrap. |
| 20 | Single-Writer Migration Protection | Missing — A's single-instance mutex is not a config lock | Normal writes and external editors can race. |
| 21 | Unique Protected Backups | Missing — no protected backup implementation | Collision/reuse not tested. |
| 22 | Optimistic Concurrency Validation | Done — `platform/config_store.rs`: every write compares the locked read with the snapshot bytes before any action or commit (`PrimaryChanged`, conflict message, then a reload); `commit_primary` re-checks the expected bytes on the held handle before the rename (2B-2c); tests `update_after_external_edit_is_conflict`, `update_with_conflict_runs_no_action`, `conflict_then_reload_then_retry_succeeds`, `multi_save_uses_last_committed`, `conflict_inside_action_reports_conflict` (Unit) | A change in the short window between the last byte check and the atomic rename is not detected. |
| 23 | Conservative Crash Recovery | Partial — S interruption and invalid-backup tests (FS) | `.new` is ignored; path-based copy has race and ACL risks. |
| 24 | Validated Recovery Selection | Partial — S rejects invalid/future rolling backups and ambiguous protected artifacts (FS) | No provenance-based protected-backup selection or handle-based validation. |
| 25 | Non-Destructive Recovery Mode | Missing — no runtime recovery mode | Existing read may create/restore before validation. |
| 26 | Validated Live Recovery | Missing — A `reload` uses existing reads | No mode transition or transactional runtime verification. |
| 27 | Minimal-Scope Migration Integration | Missing — no integration | Cannot assess integrated scope. |
| 28 | Core Logic + Platform I/O | Partial — M is pure; S owns existing I/O | Migration-specific Windows I/O missing. |
| 29 | Post-Write Verification | Missing — S `replace` does not reopen migration output | No verified commit boundary. |
| 30 | Structured Migration Failure State | Missing — no persistent state | No categorized retry metadata. |
| 31 | Clear-on-Success Failure State | Missing — no failure-state implementation | Cleanup behavior absent. |
| 32 | Transactional Schema Version Commit | Partial — M emits one prepared document (Unit) | No migration persistence transaction. |
| 33 | Version-Scoped Legacy Compatibility | Partial — A `presentation_for_config` test (Unit) | Core `Settings::read` itself still applies legacy fallback; no validated runtime modes. |
| 34 | Bounded I/O Retry | Missing — no retry mechanism | Transient failures are not retried. |
| 35 | Bounded Configuration Size | Partial — M limit; S bounded-read and growth tests (FS/Unit); rolling backup is bounded before copy | No coordinated handle protects the source from changes after validation. |
| 36 | Strict UTF-8 Validation | Partial — S bounded-read and invalid-backup tests (FS) | No recovery-mode transition; path races remain. |
| 37 | Preserve Original Line Endings | Core verified — M `bom_line_endings_and_missing_terminal_newline_are_preserved` (Unit) | No migrated file commit tested. |
| 38 | Preserve File Security Metadata | Missing — S uses standard copy/rename | ACL/security descriptor preservation unverified. |
| 39 | Reparse Point Protection | Partial — S rejects reparse attributes on the checked final file path | Check-to-open race, ancestor junctions and replacement targets remain unprotected; optional symlink test may skip. |
| 40 | Handle-Based Secure File Operations | Missing — S uses ordinary path operations | Open-handle identity and share modes not validated. |
| 41 | Durable Migration Commit | Missing — no migration commit | Existing `sync_all` is not full durability proof. |
| 42 | No-Op Migration Detection | Core verified — M `versions_are_classified_without_rewriting_current_or_future` (Unit) | No runtime check of timestamps or backups. |
| 43 | Verified Configuration Bootstrap | Missing — A `run` calls individual stored readers | No verified snapshot or migration outcome. |
| 44 | Primary Config Commit Boundary | Missing — no migration commit/verification | No distinction from auxiliary cleanup. |
| 45 | Bounded Startup Migration | Missing — no startup migration | Lock/wait/retry budget absent. |
| 46 | Validated Stage Reuse | Missing — no stage reuse implementation | Artifact ownership/content not established. |
| 47 | Ownership-Aware Temporary Cleanup | Missing — no migration cleanup protocol | Existing `.new` handling has no ownership coordination. |
| 48 | Explicit Migration Outcome | Partial — M `MigrationOutcome` (Unit) | Runtime lacks Migrated/AlreadyCurrent/FutureReadOnly/LegacyFallback/RecoveryMode. |
| 49 | Safe Core Functionality | Missing — no recovery mode | Safe feature restrictions not established. |
| 50 | Actionable User-Friendly Errors | Partial — existing native settings-save errors | Migration/recovery-specific messaging absent. |
| 51 | CI-Gated Migration Tests | Configured — C runs `cargo test --workspace` on Windows push/PR; [baseline run for `4da27e5`](https://github.com/ucanalgan/islora/actions/runs/43708310951) succeeded | This uncommitted patch has no remote run; branch protection not verified; Phase 2 tests absent. |
| 52 | Release-Ready Acceptance Gate | Missing — this audit is blocked | Critical requirements above are unimplemented. |

## Scenario matrix

| Scenario | Evidence / result |
|---|---|
| A fresh install | **Partial:** isolated test creates version 2; recovery-artifact inventory blocks several ambiguous cases, but secure coordination is missing. |
| B–D legacy mappings | **Core only:** M mapping test covers minimal, balanced and detailed. |
| E explicit modern preferences | **Core only:** M precedence test. |
| F already current | **Core only:** M returns `AlreadyCurrent`; no disk no-op test. |
| G future schema | **Partial:** M classifies future and S refuses a normally observed future file; no coordinated runtime mode. |
| H–I invalid / duplicate | **Core only:** M rejects; S normal writer refuses classified-invalid input. |
| J backup failure | **Partial:** S tests rolling backup failure; protected migration backup absent. |
| K temporary write failure | **Partial:** S tests draft creation failure for normal writes; no migration commit. |
| L interrupted migration | **Partial:** `.new` is not promoted; rolling backup is validated, but secure recovery and protected-backup selection are missing. |
| M–N concurrency / external edit | **Missing:** no shared migration/normal-write lock or source identity check. |
| O–P trusted / ambiguous backup | **Partial:** invalid or ambiguous candidates are rejected; no protected-backup selection or provenance check. |
| Q–R recovery / live repair | **Missing:** no runtime recovery states or verified transition. |
| S settings write in read-only mode | **Partial:** ordinary writer rejects an observed future schema; no mode and race protection. |
| T timer / presentation regression | **Unit only:** existing workspace tests cover timer, Quick Controls and presentation; no manual Windows verification. |

## Security and verification limitations

No real user configuration or Windows autostart is touched by the new tests; fixtures use isolated temporary directories and synthetic data. `validation_error_does_not_disclose_configuration_values` checks the returned validation error, not all possible logs or OS diagnostics. `try_remember_path` has a check-to-replace gap, and `try_read_path` can restore/create without a coordinated secure commit. Protected backups, Windows handle/share semantics, complete reparse protection and ACLs have not been implemented or verified. The symlink test returns without testing when OS privileges or filesystem support are unavailable. Hardware power-loss behavior, live Windows UI, external processes, and branch protection remain unverified.

The existing workflow runs format, workspace tests, Clippy and release build on Windows for pushes and pull requests. The Phase 1 tests are included in workspace tests. The baseline `4da27e5` run succeeded, but the current uncommitted patch has not run on GitHub. Phase 2 integration and Windows security tests do not exist yet; a passing CI run cannot satisfy this acceptance gate. This audit does not authorize a commit, push, Phase 2 completion claim, or Step 4.
