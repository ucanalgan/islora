# Islora 1.0 execution plan

This is the working plan for turning the repository into Islora 1.0. It is
derived from a read-only audit of the code on branch `v1/foundation`
(based on `main` at `d7c5314`), not from earlier claims in other documents.
Statuses change only when code lands with tests and an independent review.

All work happens on the local branch `v1/foundation`, one reviewed commit per
task, in dependency order. Merging, pushing, opening a pull request and any
destructive Git operation each need the user's explicit approval.

## How each task is done

The lead is Claude Opus: it plans, writes each task's spec, assigns workers,
reads their results, decides on corrections and makes the commits. Workers
are Codex CLI and Claude agents. Codex does most of the coding; a change is
reviewed by a reviewer the lead picks per task; the author's model may review
only in a fresh read-only run and only alongside another model kind.

**The lead does no work itself.** It never reads source files, diffs, logs or
long documents and never edits code. Anything that needs reading goes to a
read-only agent that returns a short summary (a page at most); code goes to an
implementer; checking goes to a reviewer with a fresh context. The lead keeps
only plans, specs and verdicts, so its context stays small and never compacts.
A new lead session starts every 2-3 tasks, from this file.

**Budget line.** Since 2026-11-19 (about 22:20 UTC on 11-18) the user allows
using the whole 5-hour window of both providers: the 5-hour line is 100% for
Claude and for Codex. Keep a few points in reserve for the hand-over. The
guard is Claude's weekly window, stop at 97%. Park cleanly before either line
(see `resume-handoff`).

**Pick the worker by the job and by the limits.** Before each batch, read both
limits: Claude through the session usage tool, Codex from
`rate_limits.primary.used_percent` in the newest
`~/.codex/sessions/**/rollout-*.jsonl`. Then:

- Planning, specs and final decisions: Opus, as the lead.
- Code: Codex, or a Claude Sonnet agent when Codex has less room.
- Review: the lead picks the main reviewer per task by the kind of change
  (Sonnet, Gemini 4 on `agy`, or a fresh read-only Codex run, never alone for
  Codex-written code); critical changes get at least two model kinds.
- The lead is always Opus 5.5. The kind of work decides the model; limits are
  only a brake (park near the line), never a reason to use the wrong model.
- Gates, commits and other mechanical steps: plain commands, or a Haiku or
  Sonnet agent with low effort, never an Opus agent.
- Hard architectural questions: give the same question to Opus and Codex
  separately and decide from both answers.

1. **Implement:** one worker, usually Codex
   (`codex exec -s workspace-write -C <repo> ... < /dev/null`), makes the
   change in the owned files only and does not commit.
2. **Gate:** the lead runs `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets -- -D warnings` and
   `cargo test --workspace` as plain commands, not through an agent.
3. **Review:** a worker of the other kind looks at the diff: a Claude
   reviewer for Codex's code, Codex (read-only sandbox) for Claude's. For a
   risky change both review. They check
   correctness, Rust safety, Win32 handle lifetimes, threading,
   security/privacy, persistence, error handling, performance, accessibility,
   regressions and tests.
4. **Correct:** up to three correction rounds, then a re-review. An unresolved
   critical finding stops the task: nothing is committed and it is reported.
5. **Commit:** a local commit with no attribution trailers.

Tests never touch the real `%APPDATA%\Islora`, the registry, the running
Islora or the mouse; filesystem tests use unique temporary directories.

## Phase B: secure configuration foundation (critical path)

Order: Pre-2A → 2A-1 → 2A-2 → 2A-3 → core state → 2B-2 → 2B-3 → 2C-2 → 2C-3 →
2D-2. Automatic migration stays disabled until 2A-2, 2B-2, 2B-3 and 2C-2 have
all landed.

| Task | Scope | Files |
|---|---|---|
| Pre-2A | Narrow the uncommitted gates: refuse writes only for future, invalid schema version, duplicate or malformed critical keys. The presentation reader falls back to `Settings::read` for errors unrelated to presentation. The startup autostart write is skipped when the file cannot be written. | `platform/settings.rs`, `app.rs` |
| 2A-1 | `ConfigDir` (handle to `%APPDATA%\Islora`, no reparse point), `FileIdentity` (volume + file ID), `VerifiedFile` (opened with `FILE_FLAG_OPEN_REPARSE_POINT`, rejects reparse points, directories, extra hard links, files outside the directory) with bounded reads. Existing checks rebuilt on top. Tests with symlinks, junctions and hard links. | new `platform/config_fs.rs`, `platform/settings.rs` |
| 2A-2 | `WriterLock`: process mutex + `config.lock` with share mode 0 and delete-on-close, bounded retry. Reads become side-effect free. First-run creation, rolling-backup recovery and the old-folder rename move into one `prepare_storage()` called once at startup. The calendar thread gets its address through its channel. | `config_fs.rs`, `settings.rs`, `app.rs`, `sources/calendar.rs` |
| 2A-3 | Keep the primary's owner and DACL across replacement (`GetSecurityInfo`, security attributes for temp files). | `config_fs.rs`, app `Cargo.toml` |
| Core state | Pure `island-core::config_state`: protected backup names and retention, change fingerprint, `RuntimeMode`, `StartupOutcome`, `plan_startup`, failure record encode/decode, retry decision, recovery candidate selection. Exhaustive unit tests. | new `island-core/src/config_state.rs` |
| 2B-2 | `commit_primary`: unique `CREATE_NEW` temp file with the primary's security, write-through, flush, re-read; optimistic check that the primary still has the expected identity and bytes; rename by handle with replace (fallback `MoveFileExW` with write-through); re-open and verify; delete own temp on failure. The ordinary writer goes through it. | new `platform/config_commit.rs`, `settings.rs` |
| 2B-3 | Protected backups: `CREATE_NEW` with core-generated unique names, read-only attribute, flush, verify, never overwrite, retention through the core rule. | `config_commit.rs` |
| 2C-2 | `ConfigStore::bootstrap` before any window: one verified snapshot feeds every reader; every writer goes through `ConfigStore::update`, refused in read-only and recovery modes. Migration is still mapped to legacy fallback. | new `platform/config_store.rs`, `app.rs`, `calendar.rs` |
| 2C-3 | Enable automatic migration under the lock: protected backup, commit with the classified bytes as expectation, verify, report `Migrated`; on any failure keep the original and fall back without writing. End-to-end tests for scenarios A–S in `CONFIG_MIGRATION_ACCEPTANCE.md`. | `config_store.rs` |
| 2D-2 | Recovery: artifact inventory, persisted failure record (no config values), controlled retry, validated restore, recovery mode that refuses writes and never deletes artifacts, clear on success, distinct messages for read-only and recovery. | new `platform/config_recovery.rs`, `config_store.rs`, `app.rs`, `settings_view.rs` |

## Phase C: runtime and feature correctness

Independent of Phase B except where settings keys are added.

- **Critical alerts:** indicators all share `Kind::Alert`, so a low-battery
  alert is replaced by the next volume step. Give critical alerts their own
  kind and never coalesce them away.
- **Priority:** timer completion must outrank ordinary notifications;
  separate slots for timer and focus completion.
- **Suppression:** completions and queued notices keep expiring while the
  island is hidden for full screen; hold them until the island can show.
- **Interaction:** hover keeps transient alerts and Quick Controls open.
- **Notification queue:** staleness limit, adaptive durations (4–6 s,
  6–8 s grouped, 8–12 s high priority), bounded size that never drops critical.
- **Media:** a transient metadata error must not clear the session.
- **Features still missing:** CPU and RAM indicators; wave styles Classic Bars,
  Fluid Wave, Pulse Dots, Micro Spectrum; motion styles Breathe, Drift,
  Ripple; Visible Idle. Waves are state-driven and must not be presented as
  audio analysis.

## Phase D: Windows UI integration

- **Native OSD:** the current suppression is disabled but its system-wide
  show hook stays installed for nothing; remove it until a guarded design
  exists. A guarded design needs a Windows build allowlist, a post-hide check
  that disables itself on mismatch, and a proven fail-open path after crash.
  Per-event volume versus brightness suppression is not possible with the
  current single shell host and must be documented as a gap.
- **Notifications:** report access denial and worker failure to Settings;
  bound the cache; respect Windows Do Not Disturb; hide content while locked;
  manual Privacy Mode; per-app privacy; grouping by app. Real-time events
  need package identity; until then polling stays and is documented.
- **Resilience:** panic hook and crash log without sensitive data; degrade
  instead of exiting when tray or presence registration fails; re-register
  the appbar after an Explorer restart; brightness keys update Quick Controls.

## Phase E: Settings and data management

Seven sections (Görünüm, Sistem Göstergeleri, Müzik Çalar, Etkinlikler,
Sistem ve Bildirimler, Hızlı Kontroller, Genel), resizable window with a
collapsible sidebar, remembered category and window state clamped to the work
area, System/Dark/Light theme, local ranked search, override indicators,
confirmed per-setting/feature/all resets, live try-and-commit previews,
external-edit conflict handling, read-only and recovery messaging, JSON
backup and restore with merge preview, and a health dashboard fed by the
Phase B engine. Fix found defects: modal error dialogs inside a borrowed app,
key auto-repeat toggling switches, Escape discarding the calendar address,
the muted-apps reset without confirmation.

## Phase F: accessibility, performance, resilience

UI Automation providers for the custom-drawn settings window and Quick
Controls (no screen reader can use them today), High Contrast, text scaling,
accessible announcements, keyboard-only tests. Measured idle and wave cost
budgets on real hardware. Thread lifetimes (focus, imaging, Bluetooth),
calendar download timeout and size cap, LAN slow-loris limit, removing secret
URLs from diagnostics, `SAFETY` comments for unsafe blocks.

## Phase G: release gate

Reproducible CI (pinned toolchain and actions, `--locked`), dependency audit,
checksums, installer and update strategy without silent auto-update, license,
e2e scripts that never stop the user's running copy, and real-device checks
for multiple monitors, DPI scales, full screen games, sleep and Explorer
restart. Nothing is called release-ready while a critical item is open.

## Progress

| Task | State | Commit |
|---|---|---|
| Audit and plan | done | — |
| Existing Phase 2 work kept | done | `99633f8` |
| Formatting fix for CI | done | `dc2736d` |
| Pre-2A | done: gate and presentation reader share one classifier that ignores only unrelated unsupported lines; 459 tests | see `git log` |
| 2A-1 | done: `config_fs.rs` with `ConfigDir`, `FileIdentity`, `VerifiedFile`; settings reads go through it; 471 tests | see `git log` |
| 2A-2 | done (plus review cleanup): writer lock, side-effect-free reads, `prepare_storage`, locked autostart transactions, calendar address by channel, every file operation bound to the held directory handle; 483 tests | see `git log` |
| 2A-3 | done: owner and DACL kept across replacement through exclusive drafts and pre-rename checks; 502 tests | see `git log` |
| Core state 1/3 | done: `ProtectedBackupName` (canonical build/parse), `next_sequence`, `retention_excess`, FNV-1a `ChangeFingerprint`; 507 tests | see `git log` |
| Core state 2/3 | done: `PrimaryObservation`, `ArtifactInventory`, `RuntimeMode`, `StartupOutcome`, `plan_startup` with rules a–e in precedence order; explicit 42-row matrix and precedence tests; 516 tests | see `git log` |
| Core state 3/3 | done: `FailureRecord` canonical encode/decode (no setting values, errors carry no input), `retry_decision` (corrupt record fails closed), `select_candidate` (Restore / Ambiguous / NoneValid); 521 tests | see `git log` |
| Core state cleanup | done: items above the test module (pure move), missing docs, parse and decode boundary/order tests; Sonnet: no findings | see `git log` |
| 2B-2a | done: `commit_primary` and `commit_primary_with_hook` in `platform/config_commit.rs`, pure move; Sonnet and Gemini 4: no findings; 521 tests | see `git log` |
| 2B-2b | done: drafts (primary and `.bak`) opened write-through only for draft creation; each draft read back through its own handle and compared before any rename, mismatch -> `InvalidData`, no rename, draft deleted by `Drop`; Sonnet and Gemini 4: no findings; 523 tests | see `git log` |
| 2B-2c | done: `commit_primary(dir, path, text, expected: Option<&[u8]>)`; bytes checked against the held source handle early (before any backup draft) and last in `Draft::rename`; mismatch, missing or oversized primary -> `PrimaryChanged` (`is_primary_changed`), no rename, `.bak` kept, draft deleted; Sonnet and Gemini 4 approve after one correction; 528 tests | `b1c774c` |
| 2B-2d | done: after the rename the primary is re-opened through the held directory (verified open) and must have the draft's identity and the written bytes, else `CommitUnverified` (`is_commit_unverified`); the save fails, later saves in the same locked transaction are refused, the action's own result passes through; Sonnet and Gemini 4 approve after one correction; 532 tests | `dfe7ce3` |
| 2B-2 cleanup | done: post-rename test hook cleared at every commit start, stale `dead_code` allow dropped, verify logs the error kind only, `autostart_transaction` test for an unverified save with a failed restore -> `SaveFailed(false)`; Sonnet: no findings; 534 tests | `9b94644` |
| 2B-2 conflict message | done: `SaveFailure { restored, conflict }` replaces the bare bool in `save_with_rollback` and `AutostartOutcome::SaveFailed`; conflict from `is_primary_changed` on the save error; distinct settings and calendar texts, failed restore always visible, old texts unchanged; Sonnet approves (minors only); 538 tests | `71145c1` |
| 2B-3a | done: `create_protected_backup` (core-built name over the held directory, `Draft::create_exact` CREATE_NEW of that exact name with the primary's security, write-through, read-back verify, `Draft::keep_read_only` sets READONLY by handle and restores the attributes on any later failure so drop can delete); Marco (Sonnet) wrote it, Giulia and Enzo approve after one correction; 544 tests | see `git log` |
| 2B-3b | done: `apply_protected_retention(dir, just_created, keep)` deletes only names `retention_excess` returns, never `just_created` or a non-parsing name; each excess backup opened relative to the held directory (`open_for_delete`, no reparse follow, non-directory), identity re-checked, READONLY cleared by handle, deleted by `FileDispositionInfo`, attributes restored if the delete fails, stops at the first failure; Marco (Sonnet) wrote it, Giulia and Lucia approve in one round; 552 tests | see `git log` |
| 2C-2 S1 | done: `platform/config_store.rs` (pure, not wired): `observe` (read result -> `PrimaryObservation`, explicit arm per classification, future version saturates to `u32`, fingerprint over the exact bytes), `effective_outcome` -> `RuntimeMode` (migrate becomes `LegacyCompatible`), `writes_allowed` (false only for `ReadOnly`, `Recovery`), `ModeRefused` / `is_mode_refused` (`PermissionDenied`); Paolo wrote it in one pass, Giulia and Lucia approve; 563 tests | see `git log` |
| 2C-2 S2 | done: `ConfigSnapshot` (text, mode, `storage_ready`; `Debug` hides the text), `bootstrap_at` / `bootstrap` sharing one observe path (preparation once as before, inventory from `dir.entries()`, lock-free verified read relative to the held `ConfigDir`), `install_once` slot with poison recovery; app startup calls `config_store::bootstrap()`; settings.rs gains `is_stale_draft_name` (pure extraction) and visibility changes only; readers not switched; Lucia and Enzo approve (Giulia unavailable twice); 572 tests | see `git log` |
| 2C-2 S3 | done: the 17 `settings::read()` call sites in app.rs read `config_store::text()` (parsing, defaults, `--notch`, presentation fallback unchanged); `reload()` refreshes first; `refresh` shares the observe path, does not re-prepare, copies `storage_ready`, installs nothing on an empty slot; `try_remember` / `locked_update` refresh after every write attempt (interim re-read until S4b); Lucia approves after one correction, Enzo's two majors rejected by the lead; 579 tests | see `git log` |
| 2C-2 S4a | done: `read_locked`, `try_remember_path`, `locked_update_at` moved from settings.rs to config_store.rs, pure move (mechanical removed/added check: only `use` lines and one `pub(crate)` differ); settings.rs wrappers resolve through `use`; Nino (Haiku) approves; 579 tests | see `git log` |
| 2C-2 S4b | done: `config_store::update` / `update_with` (and `_in` forms sharing `run_update`): mode gate first (`ModeRefused` for `ReadOnly`, `Recovery`, `storage_ready` false or no snapshot; no lock, no action), then `storage_writable`, lock and commit as before; write-through of the last committed text (also after a later action error), mode reclassified from a folder listing, `CommitUnverified` -> one re-read; a write sequence taken under the writer lock so an older install never replaces a newer one; settings.rs wrappers are one-line calls; Lucia and Giulia approve after one correction; 589 tests | see `git log` |
| 2C-2 S5 | done: `SaveFailure.read_only` (from `is_mode_refused`, wins over `conflict`); two read-only settings texts and one calendar text; the interactive autostart outer refusal reports the read-only text (restored, no autostart change); startup stays silent; Enzo and Nino approve (Giulia timed out); 590 tests | see `git log` |
| 2C-2 S6 | done: refresh keeps the text installed at install time on any read failure (resolved under the slot mutex after the sequence check, so a newer write-through is never overwritten); mode reclassified on every refresh (future -> `ReadOnly`, current -> `Normal`, missing primary on refresh -> `Recovery`, never new install); `storage_ready` sticky; bootstrap unchanged; Enzo (critical race found, fixed) and Lucia approve after one correction; 595 tests | see `git log` |
| 2C-2 S7 | done: writes expect the snapshot's bytes: right after the locked read and before any action or commit, a mismatch returns the `PrimaryChanged` error (nothing written, no action, no autostart change); later saves in one transaction expect the last committed text; a conflict anywhere in the transaction (also one an action swallows) makes the global `update` / `update_with` set a reload request, and `mute_shown_app`, `choose_start_mode` and `on_settings_action` reload after the rollback and message; acceptance row #22 done; Giulia approves after one correction, Lucia approves; 603 tests | see `git log` |
| 2C-2 S8 | done: dead `settings::read` removed, `try_read_path` / `verified_file` / `read_bounded` now `#[cfg(test)]` (only tests use them), stale calendar comment in app.rs fixed; `CONFIG_MIGRATION.md` Phase 2 status rewritten, acceptance rows #43 done and #48 partial, #22 risk text sharpened; Enzo and Nino found overstated doc claims (owner kept only for the current user, no snapshot comparison when the snapshot has no text), fixed, Enzo approves; 2C-2 complete; 603 tests | see `git log` |
| Phase B cleanup CL1 | done: config_store.rs minors C1-C10 (explicit expected modes and a real migrate-arm assertion, saturation bounds, pinned bootstrap-unreadable state, case-insensitive `config.toml.bak` inventory match, threading comments, low-sequence refresh test, empty-folder observe test); Lucia and Enzo: no findings; 606 tests | see `git log` |
| Phase B cleanup CL2 | done: config_fs.rs / config_commit.rs minors A1-A5 (the two `NtCreateFile` paths stay separate, with a comment saying why: security retries vs. directory mapping; restore errors after a failed delete or a failed protected backup log their kind only; the retention failure test checks sharing violation 32; one `PRIMARY_NAME` in settings.rs); Enzo and Nino approve; 606 tests | see `git log` |
| Phase B cleanup CL3 | done: test that the startup draft sweep leaves protected backups; startup autostart silent on a read-only `SaveFailed`; `PRIMARY_NAME` doc line back and used for the two non-test opens in config_store.rs; Enzo and Nino approve; Phase B cleanup complete; 607 tests | see `git log` |
| 2C-3 S1 | done: protected backups and `config.toml.failure` are non-ambiguous recovery evidence (no fresh template for a missing primary, bootstrap `Recovery`; a valid rolling `.bak` still restores next to them); the sweep also removes `config.toml.failure.<a>-<b>-<c>.tmp` drafts and nothing else; pin test flipped; Giulia and Lucia approve (minors in `scratchpad/2c3-minors.md`); 610 tests | see `git log` |
| 2C-3 S2 | done: `config_commit.rs` `read_failure` (bounded 257-byte read through the folder handle; absent -> `None`, anything unreadable, oversized, undecodable or not a regular file -> `Corrupt`), `write_failure` (Draft, verify, rename; replaces; no draft left and the old record kept on failure), `clear_failure` (idempotent); `FAILURE_RECORD_NAME`; inventory fills `recorded_failure` for `Valid` only; write / clear unused until S5; Giulia and Lucia approve in one pass; 618 tests | see `git log` |
| 2C-3 S3 | done: `settings::prepare_storage_locked(base) -> io::Result<(bool, WriterLock)>` (the old body, returns the lock built from the mutex taken at the start); `prepare_storage_at` wraps it and releases the lock before returning; no production caller until S6; tests prove held-until-drop and released-on-return from another thread (Windows mutexes are recursive per thread); Giulia approves, Enzo approves after one correction (his busy-instance point rejected: existing tests behave the same, noted in minors); 622 tests | see `git log` |

## Resume here

Pre-2A is committed. `settings::classify_ignoring_unsupported` strips only
unrelated unsupported lines and re-classifies, so future, invalid-version,
duplicate and malformed critical keys are refused wherever they appear; the
write gate and `presentation_for_config` both use it. Known limitation,
deferred to 2D-2: `recover` still rejects a `.bak` that contains unsupported
lines.

2A-1 is committed. `platform/config_fs.rs` holds `ConfigDir` (directory
handle, refuses reparse points), `FileIdentity` (volume + 128-bit file ID,
zero IDs refused) and `VerifiedFile` (handle checks for reparse point,
directory, extra hard links, containment by parent identity, strict names and
a final-name check against 8.3 aliases, bounded reads). Handles deny delete
sharing on purpose. Carried into 2A-2: open the file relative to the directory
handle (`NtCreateFile` with `RootDirectory`, marked `TODO(2A-2)`), and the
path-based replace and recovery races.

2A-2 is committed. `WriterLock` (a `Global\` mutex named from the user SID
with an owner-only ACL, plus `config.lock` with share mode 0 and
delete-on-close, about 100 ms bounded retry, `!Send`) guards every writer.
Reads have no side effects; `prepare_storage()` runs once at startup (legacy
rename, first run, backup recovery) and a transient lock there is retried on
the first write. Both autostart paths run as one locked transaction and show
dialogs only after unlock. The calendar thread gets its address through its
channel. Files are opened, enumerated and renamed relative to the held
directory handle (`NtCreateFile`, `FILE_RENAME_INFO` with `RootDirectory`), so
a redirected path cannot move a write outside it. After a runtime deletion of
the config file, saves fail instead of recreating it.

The 2A-2 review minors are fixed in a follow-up commit: no startup dialog for
an unreadable config (only a failed save shows one), the rename name pointer
taken from the base allocation, tests for invalid rename targets and the rename
fallback order, collision-free test directories, chained saves inside one
locked transaction, and a first-run flag that survives a busy preparation
(sharing errors 32 and 33 are retried too).

2A-3 is committed. Every file that replaces the primary is a uniquely named
draft created exclusively, relative to the held directory, with the primary's
descriptor (owner, group, DACL, protected bit). Its DACL and protected bit are
checked before any data is written and again before the rename. The source
(the primary, or in recovery the backup, read and captured from one held
handle) is re-checked for identity and security just before the rename and
then released, so the non-POSIX fallback rename keeps working. The owner is
kept when it is the current user (required); the group is best effort. First
run inherits from the folder, and first-run and recovery renames never
replace an existing file. Leftover drafts are swept at startup. Threat model,
documented in `config_fs.rs`: a process of the same user that races between
the final check and the rename is out of scope, because it could rewrite the
config directly. Accepted minor: a DACL mismatch right after draft creation
aborts the save (fails closed). Core state followed; see below.

Core state is parked before it started (2026-11-17): at session start Claude's
5-hour window was at 91% (weekly 85%) and Codex's at 55%, both over the 45%
stop line. No limit points were spent on it. When resuming, give it to Codex as
three single-concern steps, each committed on its own, in
`island-core/src/config_state.rs` + `lib.rs` (pure logic, no Windows
dependency):

1. Protected backup names and retention: `ProtectedBackupName` (build and
   parse), `next_sequence`, `retention_excess`, change fingerprint (fnv1a64,
   used only to notice a change, not for security). Review: Sonnet.
2. Startup decision table: `RuntimeMode`, `StartupOutcome`,
   `PrimaryObservation`, `ArtifactInventory`, `plan_startup`, with a test for
   every input combination. Invariants, in precedence order:
   a. never decide "new install" while recovery artifacts exist;
   b. a future schema is always used read-only, never migrated or written;
   c. an invalid or conflicting file opens in legacy compatibility without
      being touched;
   d. no migration retry while a failure is recorded for a source with the
      same fingerprint, until the source changes;
   e. migration is planned only when none of the above blocks it.
   Review: Sonnet, plus a read-only Gemini 4 review through Antigravity
   (`agy`, `gemini-4-hydrogen-high`; skip that round on errors and note it;
   never wait).
3. Failure record and retry: `FailureRecord` encode/decode (never contains a
   setting value), `RetryDecision`, `select_candidate` (Restore / Ambiguous /
   NoneValid). Review: Sonnet.

Step 1 is committed (Codex `gpt-6.1-terra` high wrote it in one pass, gate
green, Sonnet review approved). Minors collected for the end-of-phase cleanup:
add rejection-table cases for uppercase prefix/suffix (`Protected`, `.BAK`,
`V3`), a `-` sign and inner whitespace; document that `retention_excess`
orders equal sequences by schema and fingerprint (only reachable with a
planted file, since `next_sequence` never repeats a number).

Budget record for core state step 1: Claude 5-hour 35 → 38, Codex 5-hour
0 → 1 (one Codex run, one Sonnet review, no correction round).

Step 2 is committed. Codex wrote it in one pass; Sonnet and Gemini 4
(`agy`, `gemini-4-hydrogen-high`) both found the same defect: Codex had added
a BOM and turned two step-1 non-ASCII test strings into mojibake. Rated major
(an existing test lost its meaning); one Codex correction run restored them as
`\u{..}` escapes. Neither reviewer found a problem in the decision logic or the
42-row table. Minor for the cleanup: doc comments on `ArtifactInventory`
fields and on `mode`, `allows_writes`, `has_recovery_artifacts`.

Budget record for core state step 2: Claude 5-hour 38 → 41, Codex 5-hour
1 → 2 (two Codex runs, one Sonnet review, one Gemini 4 review on agy).

Step 3 is committed. Codex wrote it in one pass, the gate was green (no BOM,
ASCII only, no removed lines) and the Sonnet review approved it. More minors
for the cleanup: test the 256/257-byte boundary (256 bytes must be Malformed);
test the error order (257 bytes of invalid UTF-8 is TooLarge; non-UTF-8 after
a version-2 line); and move the non-test items that steps 2 and 3 appended
after `mod tests` above it (the spec said "append").

Budget record for core state step 3: Claude 5-hour 41 → 45, Codex 5-hour
2 → 3 (one Codex run, one Sonnet review). Whole core state: Claude 35 → 45,
Codex 0 → 3, Claude weekly 90 → 91. Each narrow step cost about 3 Claude
points.

Parked 2026-11-18 at the stop line: Claude 5-hour 45% (resets 11:10 UTC),
weekly 91% (resets 16:00 UTC); Codex 5-hour 3%, weekly 9%. Working tree
clean apart from the untracked `docs/V1_STABLE_CHECKLIST.md`,
`docs/WAVE_MEASUREMENTS.md` and `scratchpad/`, none of which are part of
this work. First step next: one Codex run for the cleanup commit covering
every core state minor listed above (tests, doc comments, item order), with a
Sonnet review; then 2B-2. Keep Codex steps narrow and tell Codex the file
hygiene rules (no BOM, ASCII escapes, no removed lines) in every prompt.

Core state cleanup is committed (one Codex run, gate green, every removed
line re-added unchanged, Sonnet: no findings). Budget: Claude 5-hour 46 → 49,
weekly 91 → 92; Codex 3 → 3.

2B-2 plan, from a read-only Gemini 4 (`agy`) scan of `config_fs.rs` and
`settings.rs`: the unique `CREATE_NEW` draft with the primary's descriptor
and deletion of the own draft on failure already exist (`Draft::create`,
`Drop for Draft`); write-through, draft read-back, expected-bytes check and
post-rename verification are missing or partial. Narrow steps, each its own
commit, each reviewed by Sonnet and Gemini 4 on `agy`:
- 2B-2a: move `replace_in`, `replace_in_with_hook`, test-only `replace` and
  `name` from `settings.rs` into new `platform/config_commit.rs` as
  `commit_primary`; settings writers call it. Pure move, no behaviour change.
- 2B-2b: drafts opened write-through; the draft is read back and compared
  byte for byte before the rename.
- 2B-2c: optimistic check before the rename: the primary still has the
  expected identity and the expected bytes (the bytes the caller read under
  the lock); otherwise abort and delete the draft.
- 2B-2d: after the rename, re-open the primary relative to the directory and
  verify it is the draft's file identity with the written bytes.
Deviation from the 2B-2 row: no path-based `MoveFileExW` fallback. 2A-2 bound
every file operation to the held directory handle so a redirected path cannot
move a write outside it; a path-based fallback would break that, and the
newer security invariant wins. The existing handle-based fallback
(`FileRenameInformation` after `FileRenameInformationEx`) stays.

Budget record for 2B-2a: Claude 5-hour 49 → 51, weekly 92; Codex 3 → 4 (one
Codex run, Sonnet and Gemini 4 reviews, no correction).

Budget record for 2B-2b: Claude 5-hour 51 → 53, weekly 92; Codex 4 → 5 (one
Codex run, Sonnet and Gemini 4 reviews, no correction). Narrow 2B-2 steps cost
about 2 Claude points and 1 Codex point each.

Parked 2026-11-18 after 2B-2b because the lead's context passed 50% (64% of a
400k window), not because of limits: Claude 5-hour 53% (resets 11:10 UTC),
weekly 92% (guard 97%, resets 16:00 UTC); Codex 5-hour 5%, weekly 9%.
Working tree clean apart from the untracked `docs/V1_STABLE_CHECKLIST.md`,
`docs/WAVE_MEASUREMENTS.md` and `scratchpad/` (review copies only).
First step next: 2B-2c in a new lead session. `commit_primary` gets the
expected bytes (what the caller read under the writer lock) and, right before
the rename, re-reads the primary through the held source handle and checks
both identity and bytes; on a mismatch it aborts with a distinct error and
the draft is deleted. Callers in `settings.rs` pass the bytes they read.
Then 2B-2d (post-rename re-open and verify). Each step: Codex writes (state
the file hygiene rules), lead runs the gate plus the BOM/ASCII/removed-lines
check; the lead names the reviewers per step (2B-2c/d are critical, so at
least two model kinds, Gemini 4 on `agy` among them).

2B-2c is committed (`b1c774c`). The first spec kept `config_fs.rs` out of the
owned files, so Codex stopped without editing (the final checks live in
`Draft::rename`, and `VerifiedFile` does not expose identity). Revision 2 made
`config_fs.rs` owned, put the bytes check in `Draft::rename`, and dropped the
caller's expected identity: the existing held-source re-check enforces it
(`replaced_source_aborts_replacement`), and a swap to identical bytes needs a
same-user racer, which the threat model excludes. Chained saves pass the text
the previous save wrote; first run and recovery pass `None`. Gemini 4 found
two majors (an external invalid-UTF-8 edit gave `InvalidData`, and a refused
save had already replaced `.bak`); one Codex correction added the early check
before `read_verified` and any backup draft. Re-review: Sonnet and Gemini 4
approve. Minors for the phase cleanup: `is_primary_changed` is used only by
tests (`#[allow(dead_code)]`), so wire it into a caller that shows a conflict
message; an edit in the tiny window between the early and final checks still
lets `.bak` take the pre-edit primary text (accepted, within the threat model);
an atomic editor save that swaps identity after the early check would surface
as the generic source-changed error, not `PrimaryChanged`.

Budget record for 2B-2c: Claude 5-hour 57 -> (reset at 11:10 UTC) 7, weekly
93 -> 96; Codex 5-hour 5 -> 13, weekly 10 -> 11 (three Codex runs: one that
stopped on the spec, one implementation, one correction; two Sonnet and two
Gemini 4 reviews). A critical step with one correction costs about 3 weekly
Claude points and 8 Codex points.

Parked 2026-11-18 at about 11:20 UTC on the weekly guard: Claude weekly 96%
(guard 97%, resets 16:00 UTC), 5-hour 7%; Codex 5-hour 13%, weekly 11%. 2B-2d
would cross the guard. Working tree clean apart from the untracked
`docs/V1_STABLE_CHECKLIST.md`, `docs/WAVE_MEASUREMENTS.md` and `scratchpad/`
(specs, diffs and review outputs only). First step next, after the weekly
reset: 2B-2d in a new lead session. Owned files from the start:
`config_commit.rs`, `config_fs.rs` and, for tests, `settings.rs`. After the
primary rename, re-open the primary relative to the held directory and verify
it is the draft's file identity (capture it from the draft handle before the
rename) with exactly the written bytes; on a mismatch return a distinct error,
and say in the spec what the caller does then (the rename already happened).
Reviewers: Sonnet and Gemini 4 on `agy`, both from the first round.

The user raised the weekly guard to 100% at 11:25 UTC and 2B-2d ran at once.
2B-2d is committed (`dfe7ce3`). The identity is captured from the draft's own
handle before the rename (`Draft::identity`; `FileIdentity` and
`VerifiedFile::identity` are now `pub(crate)` and no longer test-only). Only
the primary replacement is verified; backup, first run and recovery are
unchanged (more than a one-line reuse). Sonnet and Gemini 4 both found one
major: `locked_update_at` replaced the action's result with
`Err(CommitUnverified)`, so the autostart flow reported "restored" after a
failed restore, or hid the failure. One Codex correction returns the action's
result unchanged and keeps refusing later saves in the transaction.
Re-review: both approve. Minors for the phase cleanup: a direct
`autostart_transaction` test with an unverified commit and a failed restore
(the function is private to `app.rs`); clear the test-only
`AFTER_PRIMARY_RENAME` hook at the start of `commit_primary_with_hook` so an
early failure cannot leave it armed; drop the stale `#[allow(dead_code)]` on
`is_commit_unverified`; optionally log the cause hidden by `.ok()` in the
verify.

Budget record for 2B-2d: Claude 5-hour 7 -> 14, weekly 96 -> 97; Codex 5-hour
13 -> 15, weekly 11 (two Codex runs, two Sonnet and two Gemini 4 reviews).

Parked 2026-11-18 at 11:40 UTC after two tasks (2B-2c, 2B-2d), per the
2-3 task hand-over rule: Claude 5-hour 14%, weekly 97% (resets 16:00 UTC);
Codex 5-hour 15%, weekly 11%. Working tree clean apart from the untracked
`docs/V1_STABLE_CHECKLIST.md`, `docs/WAVE_MEASUREMENTS.md` and `scratchpad/`.
2B-2 is complete. First step next, in a new lead session after the weekly
reset: the 2B-2 cleanup commit (one Codex run, Sonnet review) covering the
2B-2c and 2B-2d minors above, including wiring `is_primary_changed` into a
caller that shows a conflict message; then the next Phase B row. In specs,
name every file the change truly needs as owned from the start; both 2B-2c
and 2B-2d lost a run or a test to a too-narrow owned list.

2B-2 cleanup is committed (`9b94644`): one Codex run, gate green, one Sonnet
review with no findings. Not done here, kept for its own step: wire
`is_primary_changed` into the save-failure path so a conflict with an
external edit shows its own message (UI text in `app.rs`).

Budget record for 2B-2 cleanup: Claude 5-hour 25 -> 28, weekly 98 -> 99;
Codex 5-hour 15 -> 1 after its reset (about 1 point).

Parked 2026-11-18 at 13:40 UTC on the weekly window: Claude weekly 99%
(resets 16:00 UTC), 5-hour 28%; Codex 5-hour 1%, weekly 11%. Working tree
clean apart from the untracked `docs/V1_STABLE_CHECKLIST.md`,
`docs/WAVE_MEASUREMENTS.md` and `scratchpad/`. First step next, in a new
lead session after 16:00 UTC: the conflict-message step above (owned
`app.rs` and `platform/settings.rs`; Sonnet review), then the next Phase B
row. Put the weekly guard back to 97% for the new week unless the user says
otherwise.

The conflict-message step is committed (`71145c1`). A read-only Codex scout
mapped the save-failure path; Codex (`gpt-6.1-terra` high) wrote it in one
pass; gate green (fmt, clippy, 538 tests), no BOM, new non-ASCII text as
`\u{..}` escapes, the two old texts moved byte for byte. Sonnet approved.
Minors for the Phase B cleanup: an `autostart_transaction` test with a
PrimaryChanged save error (asserts `restored: true, conflict: true`); rename
bindings still called `restored` that now hold a `SaveFailure` (`app.rs`
around 2110 and 3763, `settings.rs` around 526); one-line docs on
`calendar_save_note` and `persistence_failure_message`. The user chose to run
this step over the stop line (Claude 5-hour 63%, Codex 5-hour 62% at start).

Budget record for the conflict-message step: Claude 5-hour 63 -> 67, weekly
9; Codex 5-hour 62 -> 63, weekly 25 (one Codex scout, one Codex
implementation, one Sonnet review, no correction).

Parked 2026-11-18 at about 21:20 UTC (00:20 local, 2026-11-19) after this step: Claude 5-hour 67%
(resetting now), weekly 9%; Codex 5-hour 63% (resets about 00:30 UTC), weekly
25%. Working tree clean apart from the untracked
`docs/V1_STABLE_CHECKLIST.md`, `docs/WAVE_MEASUREMENTS.md` and `scratchpad/`.
First step next: the next Phase B row (2B-3 or whichever follows 2B-2 in the
Phase B list), as narrow Codex steps; Codex needs its window to fall below
45% first. Weekly guard for this week: 97%.

Worker names (fixed, approved by the user 2026-11-19; use them in prompts,
reports and cost lines): Paolo = Codex author, Marco = Sonnet author, Lucia =
Sonnet reviewer, Giulia = Gemini 4 on `agy` reviewer and tie-breaker, Enzo =
read-only Codex reviewer and scout, Nino = Haiku or Gemini flash scans, Sofia =
Opus for hard architecture questions. The lead is Augusto.

2B-3 is split in two: 2B-3a creates one protected backup, 2B-3b applies
retention. 2B-3a is committed: Marco wrote it (Codex was at 63%). Giulia and
Enzo found the same major independently: after READONLY was set, a failed
read-back left an undeletable orphan; one correction restores the attributes
before returning. Re-review: Giulia no findings. Enzo raised a critical
(`VerifiedFile::open_source` re-opens the parent by path) rejected as
pre-existing: `commit_primary` uses the same call and the re-open is the 2A-1
containment check by identity, not a write path; and a major (restore of the
attributes can also fail -> read-only orphan) downgraded to minor: a double
fault, and the leftover is a valid verified backup that 2B-3b retention will
remove. Minors for the Phase B cleanup: those two, plus a test that the
startup draft sweep leaves protected backups (the sweep is private in
`settings.rs`; by reading, its pattern never matches the backup names), and
the `PRIMARY_NAME` constant in `config_commit.rs` duplicates the name in
`settings.rs`.

Budget record for 2B-3a: Claude 5-hour 7 -> 19, weekly 10 -> 12; Codex
5-hour 63 -> 65 (two Enzo reviews); one Marco run plus one correction, two
Giulia and two Enzo reviews. A Sonnet-written critical step costs about 12
Claude 5-hour points, against about 3 when Codex writes.

Lead context at 45% after three tasks this session: hand over to a new lead
session before 2B-3b.

First step next: 2B-3b retention: `retention_excess` over the parsed
protected backups picks what to delete; clear READONLY by handle and delete
relative to the held directory; never delete the backup just created or any
file that does not parse as a protected backup name. Owned
`config_commit.rs` and `config_fs.rs`. Reviewers: Giulia plus Lucia or Enzo.

2B-3b is committed. Marco wrote it in one pass (Codex was at 65%, the user
chose Marco); gate green (fmt, clippy, 552 tests, no BOM, ASCII only, no
removed lines). Giulia: no findings. Lucia approves with minors for the Phase
B cleanup: `open_for_delete` repeats the `NtCreateFile` setup of
`open_relative` (factor it out or say why); a failed attribute restore after a
failed delete is dropped silently (report both); and
`retention_failure_leaves_file_and_later_files` asserts only `is_err()`
(check for a sharing or access error). Deviations accepted: the final-name
check reuses `VerifiedFile::open_source` by identity, which also refuses a
backup with extra hard links; no `SYNCHRONIZE` on the delete handle.
2B-3 is complete. `apply_protected_retention` has no caller yet (2C-3).

History rewrite (user request, 2026-11-19): `main`'s tip `9152ce5` still had
a Claude co-author trailer. It was rebuilt without it as `d7c5314`, and the 37
`v1/foundation` commits on top were re-parented with identical trees, authors,
dates and messages (old `332b0ca` -> `c87bc0d`); both branches force-pushed
with lease. Hashes in this plan are updated; older ones are in
`backup/pre-coauthor-fix-2-main` and `backup/pre-coauthor-fix-2-foundation`
(local only).

Budget record for 2B-3b: Claude 5-hour 32 -> 44, weekly 13 -> 15; Codex
5-hour 65 (no Codex run). One Marco run, one Giulia and one Lucia review, no
correction. Marco-written steps keep costing about 11-12 Claude points.

2C-2 design (2026-11-19). Three Sonnet scouts mapped the readers, writers and
startup (`scratchpad/2c2-map-*.md`, not tracked); Sofia (Opus), Enzo (Codex)
and Giulia (Gemini 4) split the task independently
(`scratchpad/2c2-wf-3.md` Enzo, `-4.md` Giulia, `-5.md` Sofia). The lead's
decisions:
- Store ownership: a process-wide store in `platform/config_store.rs` (UI
  thread only, `Mutex<Option<..>>`, poison -> `into_inner`) handing out
  `Arc<ConfigSnapshot>`; every function has a `*_at(base)` / instance form so
  tests never touch the global. Not an `App` field: about 20 write call sites
  pass rollback closures that borrow `&mut self` (Sofia; Giulia and Enzo
  missed it). The store mutex is never held across an action, commit,
  rollback or MessageBox (re-entrancy through `with_app`).
- Calendar: no store on the worker; the UI thread sends the address from the
  snapshot over the existing channel. `calendar.rs` needs no code change.
- Modes: `Migrate` maps to `LegacyCompatible`; writes are refused only in
  `ReadOnly` and `Recovery`, as the 2C-2 row says (Enzo wanted legacy refused
  too; rejected: it would freeze every v1 user's settings until 2C-3). Note
  for 2C-3: `config_state.rs` docs say legacy runs "without modifying".
- Refusal: typed `ModeRefused` (`is_mode_refused`), checked before the
  WriterLock and before any action closure (no autostart change); interactive
  callers get a distinct message, startup stays silent.
- Snapshot after a write: the committed, verified text (write-through), not a
  new read. A failed fresh read keeps the previous snapshot; defaults never
  overwrite live state. Reads are lock-free through `VerifiedFile`.
- Bootstrap: `prepare_storage` stays as is and runs once; if it fails,
  bootstrap still observes through `ConfigDir` so a missing primary with
  artifacts becomes `Recovery`, and writes stay refused (Giulia). The calendar
  address never reaches logs or `Debug` output.
- Known boundary, not 2C-2: `prepare_storage` still creates the folder and
  renames the legacy folder by path before the directory handle exists
  (Enzo); record it for Phase B cleanup.

2C-2 steps, in order, each one commit (Paolo writes; reviewers Giulia plus
Lucia or Enzo; Enzo never when Paolo wrote alone without another kind):
1. S1 pure classifier: new `config_store.rs` + `platform/mod.rs`. `observe`
   (read result -> `PrimaryObservation` via `classify_ignoring_unsupported`),
   `effective_outcome` (never `Migrate`), `writes_allowed`, `ModeRefused`.
   No wiring. Tests per Sofia's S1 list.
2. S2 bootstrap: `config_store.rs`, `app.rs` (replace the bare
   `prepare_storage()` call). Inventory from `dir.entries()`; one lock-free
   verified read; store the snapshot. Readers not switched yet.
3. S3 readers: `app.rs`. Every `settings::read()` in `app.rs` reads the
   snapshot; `reload()` refreshes it once. Keep the `--notch` override and
   the presentation fallback order.
4. S4a pure move of the write core (`try_remember_path`, `locked_update_at`,
   `read_locked`) into `config_store.rs`; `settings.rs` keeps thin wrappers.
5. S4b mode gate and write-through in `update` / `update_with`.
6. S5 refusal message: `SaveFailure` gains `read_only`; settings and calendar
   texts; startup autostart silent on refusal.
7. S6 refresh semantics: failed refresh keeps the snapshot; reclassify on
   refresh (v3 -> ReadOnly, back to v2 -> Normal, deleted primary ->
   Recovery); a prepare failure stays refused until restart.
8. S7 expected bytes = snapshot bytes (Giulia and Enzo): an external edit
   without reload gives `PrimaryChanged`, rollback, the conflict message and a
   reload. Behaviour change; acceptance row #22.
9. S8 cleanup and docs: drop dead `settings::read`, update
   `CONFIG_MIGRATION.md` "Not yet done" and acceptance rows #43 and #48.
Budget: the design round cost Claude 5-hour 44 -> 56 (three Sonnet scouts, one
Opus design, Haiku runners for Codex and agy). Codex steps should cost about
3 Claude points each plus reviews.

First step next: S1 above, in a new lead session (this lead's context passed
48%). Then the Phase B cleanup commit with every collected minor after 2C-2.

Codex never reviews these steps, because it writes them. If a major finding is
still open after two correction rounds, split the step or sharpen its spec.

2C-2 S1 is committed. Paolo (Codex `gpt-6.1-terra`, high) wrote it in one
pass; gate green (fmt, clippy, 563 tests, no BOM, ASCII only, no removed
lines). Accepted deviation: `effective_outcome` returns `RuntimeMode`, since
the core `LegacyCompatible` outcome cannot carry migration data; the real key
is `config_version`. Giulia: no findings. Lucia approves with minors for the
Phase B cleanup: `migrate_never_returned` recomputes its expected value with
the same match (also assert that a legacy case really hits the migrate arm);
saturation test should add `u32::MAX` and `u32::MAX + 1`. Reviews run as one
workflow (Haiku runner for agy, Sonnet for Lucia). Spec and prompt:
`scratchpad/spec-2c2-s1.md`, `prompt-2c2-s1-review.md`.

Budget record for 2C-2 S1: Claude 5-hour 59 -> 66, weekly 17 -> 18; Codex
5-hour 65 -> 67, weekly 26.

2C-2 S2 is committed. Paolo stopped twice for scope (correct): the inline
stale-draft match was extracted into `settings::is_stale_draft_name` (same
rule), and `mod config_store` became `pub(crate)`. Today's preparation does
not treat protected backups as recovery evidence: a missing primary with only
a protected backup gets a fresh primary and `Normal`. No caller creates
protected backups yet, so this is recorded for 2C-3 (preparation must treat
them as recovery evidence before migration creates any); a test pins today's
behaviour. The preparation-failure test uses an invalid rolling backup
(`Recovery`). Giulia (agy) failed twice with a remote connection error, so
Enzo (fresh read-only Codex) reviewed with Lucia: Enzo no findings; Lucia
approves with minors for the Phase B cleanup: pin `storage_ready`,
`writes_allowed()` and unchanged bytes in `bootstrap_unreadable_gives_none_text`;
the inventory matches `config.toml.bak` case-sensitively while settings.rs
`artifacts()` lowercases (use `eq_ignore_ascii_case`). Codex review prompts
must allow read-only shell reads (Codex reads files through the shell).

Budget record for 2C-2 S2: Claude 5-hour 66 -> 73, weekly 18 -> 19; Codex
5-hour 67 -> 70, weekly 26 -> 27 (three Paolo runs, two Enzo runs).

Night run (user, 2026-11-19 ~01:50 local): use both 5-hour windows up to
100%, wait for each reset (Codex ~23:48 UTC, Claude ~02:20 UTC), keep going
until 06:00 local (03:00 UTC); stay in this lead session (background turns
cannot open a new one); pick reviewers per step by the kind of change, not a
fixed pair.

2C-2 S3 is committed. Paolo wrote it, one correction round (Lucia's
accepted findings: a test that `storage_ready: true` is copied; refresh after
every write attempt, Ok or Err, so an unverified commit cannot leave the
snapshot older than the file; no install on an empty slot so `bootstrap`
still prepares; one shared `config_folder()`; a folder-open failure always
gives `storage_ready: false`). Reviewers chosen by kind: Lucia (Rust logic)
and Enzo (lock and ordering detail); Giulia skipped (agy connection errors).
Lead rejected Enzo's two majors: (1) refresh after `Ok(SaveFailed)` is
correct for an interim disk re-read (atomic rename, snapshot = disk), and
S4b's commit-only write-through replaces it; (2) a write refused before any
attempt touches no file, and external edits are picked up by `reload()` by
design. Minors for the Phase B cleanup: concurrent refreshes can swap an
older observation in (store is UI-thread only; add a generation check if a
second thread ever refreshes); `folder_open_failure_clears_storage_ready`
should name its failure cause. For S6: `open_settings` now shows the
snapshot's calendar address, so an editor change without Reload shows the old
address (before S3 it read the file); decide whether `open_settings`
refreshes first.

Budget record for 2C-2 S3: Claude 5-hour 73 -> 81, weekly 19 -> 20; Codex
5-hour 70 -> 73, weekly 27 (two Paolo runs, two Enzo runs, two Lucia runs).

2C-2 S4a is committed (pure move; the gate compared removed and added lines
mechanically, then Nino checked bodies, wrappers and name resolution: no
findings). Spec `scratchpad/spec-2c2-s4a.md`.

2C-2 S4b is committed. Paolo wrote it, one correction round: Lucia's major
(two overlapping in-process writers could swap an older text in after a newer
one) fixed with a write sequence taken under the writer lock; bootstrap,
refresh, write-through and the unverified re-read all install only when
newer. Tests split into the spec names, plus a `PrimaryChanged` conflict case
and `older_write_through_never_replaces_newer`. Giulia: one minor, then no
findings; Lucia approves. Minors for the Phase B cleanup: `install_once` now
runs `make` outside the slot mutex, so two concurrent `bootstrap()` calls
would both prepare (startup is single-threaded; say so in a comment or guard
with a `OnceLock`); a test that a low-sequence `refresh` after a newer install
leaves the slot unchanged; `legacy_compatible_still_writes` should assert the
mode stays `LegacyCompatible`. Behaviour change for S5: a refused write now
returns `ModeRefused` (`PermissionDenied`), callers roll back and show the
generic failure text until S5 adds the distinct one.

Budget record for 2C-2 S4a and S4b: Claude 5-hour 81 -> 94, weekly 20 -> 22;
Codex 5-hour 73 -> (reset at 23:48 UTC) 2, weekly 27.

Parked at 00:20 UTC on 2026-11-19: Claude 5-hour at 94%, resets 02:20 UTC.
The lead waits for the reset, then continues with S5.

2C-2 S5 is committed. Paolo wrote it in one pass while Claude was parked;
reviewed without Claude-heavy runs: Enzo (fresh read-only Codex) no findings;
Giulia failed twice (empty output when run in parallel with Codex, then a
9-minute timeout); Nino (Haiku) checked the decoded texts and literals and
approves. Not tested (need the global store): a pre-action autostart refusal
and the silent startup path; both checked by reading. Minors for the Phase B
cleanup: `start_with_windows_by_default` should also return silently on
`Ok(SaveFailed)` with `read_only` set (unreachable today); the interactive
refusal goes through the existing outer-`Err` branch, not
`AutostartOutcome::SaveFailed` (same text and rollback). Run agy alone, never
in parallel with another CLI.

2C-2 S6 is committed (after the Claude 5-hour reset at 02:20 UTC). Lead
decisions: a missing primary on refresh is `Recovery` whatever the inventory
(the file existed at startup; live recovery is acceptance row #26, not done);
`open_settings` does NOT refresh first (the window would show values the
running island does not use; the user presses Reload), so the S3 note is
closed as intended behaviour. One correction round: Enzo found a critical
race (a failed refresh carried a text captured before observing and could
overwrite a newer write-through); fixed by resolving the kept text at install
time under the slot mutex after the sequence check; tests for that order, a
directory named `config.toml`, and folder-open keep-text. Minor for the Phase
B cleanup: `bootstrap_missing_primary_still_new_install` goes through
preparation, which creates the file first, so it does not exercise the
`Missing` observation; test the observe path on an empty folder directly.

Budget record for 2C-2 S5 and S6: Claude 5-hour 94 -> 96 (S5, Nino only),
then (reset 02:20 UTC) 0 -> 13, weekly 22 -> 24; Codex 5-hour 2 -> 7, weekly
27 -> 28.

2C-2 S7 is committed. Lead decision (2026-11-19, previous lead): S7 is part
of 2C-2, because acceptance row #22 was still Missing. A first attempt
stopped twice for placement (there is no single dispatch) and once on a
network error; the spec then named the three app.rs places. Paolo wrote it,
one correction round: Giulia's major (an action such as
`autostart_transaction` can catch a save's `PrimaryChanged` and return
`Ok(SaveFailed { conflict: true })`, so no reload was requested) fixed by
tracking a conflict flag through the transaction. Lucia approves; minors for
the Phase B cleanup: a startup conflict (`start_with_windows_by_default`,
which has no reload check) leaves the reload request set, so the next
settings action reloads once (harmless; clear it after the startup write if
unwanted). Spec `scratchpad/spec-2c2-s7.md` and `-s7-r1.md`.

Budget record for 2C-2 S7: Claude 5-hour 10 -> 23, weekly 26 -> 27; Codex
5-hour 8 -> (reset) 2, weekly 28 -> 29 (two Paolo runs, three agy runs, two
of them network errors, two Sonnet runs, one a network error).

2C-2 S8 is committed; 2C-2 is complete. Paolo stopped once, correctly:
removing `read` left `verified_file` and `read_bounded` used only by tests;
lead decision: mark them `#[cfg(test)]` like `try_read_path` (their tests
still cover `VerifiedFile` through a path). `calendar.rs` does not exist (the
calendar code lives elsewhere), so Sofia's calendar doc line was dropped.

Budget record for 2C-2 S8: Claude 5-hour 23 -> 31, weekly 27 -> 28; Codex
5-hour 2 -> 4, weekly 29 (two Paolo runs, two Enzo runs, one Nino run).

Phase B cleanup: every collected minor is listed, grouped into three narrow
steps by file, in `scratchpad/phaseb-cleanup-minors.md` (CL1 config_store.rs
tests and comments, C1-C10; CL2 config_fs.rs and config_commit.rs, A1-A5;
CL3 settings.rs and app.rs, B1 and D1), with the lead decisions on what is
not in it. The `prepare_storage` by-path boundary moves to 2C-3 (a real
change to startup preparation, not a minor).

Phase B cleanup CL1 is committed (one Paolo run, Lucia and Enzo: no findings).
Phase B cleanup CL2 is committed; Nino's two minors (restore the dropped `PRIMARY_NAME` doc line; two `"config.toml"` literals in config_store.rs) join CL3.
Phase B cleanup CL3 is committed; the Phase B cleanup is complete (CL1
`b579989`, CL2 `ff77a9b`, CL3 next commit). Nino's CL3 minor (the test
backup has placeholder bytes) rejected: the sweep decides by name only.

Budget record for 2C-2 S8 hand-over through CL3: Claude 5-hour 31 -> 39,
weekly 28 -> 29; Codex 5-hour 4 -> 9, weekly 29 -> 30 (three Paolo runs,
three Enzo runs, one Lucia, two Nino).

Still open from Phase B, moved to 2C-3: `prepare_storage` creates the folder
and renames the legacy folder by path before the directory handle exists;
preparation must treat protected backups as recovery evidence before
migration creates any (2C-2 S2 note).

2C-3 design (2026-11-19). A Sonnet scout mapped the code
(`scratchpad/2c3-map.md`); Sofia (Opus), Enzo (Codex) and Giulia (Gemini 4)
split the task independently (`scratchpad/2c3-wf-{sofia,enzo,giulia}.md`),
a Sonnet agent compared them (`scratchpad/2c3-compare.md`). Agreed by all:
migration only in `bootstrap` / `bootstrap_at`, one WriterLock held from
preparation through inventory, locked read, migrate and snapshot; order:
locked bytes -> `retry_decision` -> strict `prepare` -> protected backup ->
`commit_primary(Some(bytes))` -> verify -> clear failure record -> retention;
exact bytes compared under the lock (a change -> no write, no record,
re-observe); retention or record-clear errors never downgrade `Migrated`; no
rollback write; refresh and writes never migrate; a one-shot migration report
on the snapshot; e2e A-S on temp bases and local slots only; the by-path
rename / folder create stays under the WriterMutex (documented limit). Lead
decisions on the disputes:
- D-A protected backups count as non-ambiguous recovery evidence (like
  `.bak`): they block the fresh template, a valid rolling `.bak` still
  restores (Sofia, Giulia; Enzo's "ambiguous" would break the `.bak` restore
  after every migration).
- D-B failure record file `config.toml.failure`; its drafts must match the
  stale-draft sweep (Sofia found the sweep matches only
  `config.toml.<n>-<n>-<n>.tmp`).
- D-C writes stay allowed after a fallback (`LegacyCompatible`, as in 2C-2;
  a save changes the fingerprint, so the next start retries).
- D-D a legacy file `prepare` refuses (unsupported line) falls back without a
  record (no write; core has no Prepare stage); 2D-2 may revisit.
- D-E `CommitUnverified`: record stage Verify, re-read the disk text into the
  snapshot, keep the protected backup (consistent with S4b).
- D-F new module `platform/config_migrate.rs` (config_store.rs is large).
- D-G production switch LAST, after the e2e steps (Enzo): until then
  `bootstrap_at` takes an explicit migrate flag that tests set and
  production passes as off.
- D-H scenario R (live repair) is pinned to today's behaviour (Recovery until
  restart); live recovery is 2D-2.
- D-I migrate reads the failure record itself; Corrupt suppresses the retry
  and is never turned into an absent fingerprint.
- D-J lock busy or preparation failed: lock-free observe, no migration,
  report `Busy` / `Unprepared`. Tests that take the per-user WriterMutex hold
  `TEST_WRITER_SERIAL` and follow `islora-safe-testing`.
2C-3 steps (each one commit, Paolo writes; Giulia plus Lucia or Enzo review):
S1 settings.rs (+config_store.rs inventory): protected backups and the failure
record as recovery evidence (D-A), sweep failure drafts (D-B), flip the pin
test. S2 failure record I/O (None / Valid / Corrupt, bounded, Draft + verify
+ rename, clear); inventory sets `recorded_failure` for Valid only. S3
`prepare_storage_locked` returns the WriterLock, `prepare_storage_at` wraps
it. S4 `config_migrate.rs`: report types, `migrate_locked`, D-D. S5 failure
recording and the post-rename path (D-E). S6 bootstrap under one lock with
the migrate flag off in production (D-G, D-J). S7 e2e A-I. S8 e2e J-S (R
per D-H). S9 the switch. S10 docs (config_state.rs text, acceptance rows and
gate). Test names per step in `scratchpad/2c3-compare.md` section 3.

2C-3 S1 is committed (one Paolo run, no correction).

Budget record for the 2C-3 design round and S1: Claude 5-hour 39 -> 62
(Sonnet scout, Opus design, Haiku runners, Sonnet compare: about 20 points;
S1 reviews about 3), weekly 29 -> 32; Codex 5-hour 9 -> 11, weekly 30.

Parked at 11:40 UTC on 2026-11-19: lead context 65%, Claude 5-hour 62%
(resets 14:10 UTC), Codex 11%. Working tree clean (only the untracked
`docs/V1_STABLE_CHECKLIST.md`, `docs/WAVE_MEASUREMENTS.md`). Collected 2C-3
minors: `scratchpad/2c3-minors.md`.

2C-3 S2 is committed (one Paolo run, no correction; Lucia's minor in `scratchpad/2c3-minors.md`).

Budget record for S2: Claude 5-hour 65 -> 78 (lead turns plus one Sonnet review; lead turns are most of it), weekly 33 -> 35; Codex 5-hour 11 -> 12, weekly 30 (one Paolo run); Giulia one agy run.

2C-3 S3 is committed (one Paolo run plus one correction).

Budget record for S3: Claude 5-hour 78 -> 85, weekly 35 -> 36 (lead turns only; reviews on Giulia and Enzo); Codex 5-hour 12 -> 16, weekly 31 (two Paolo runs, two Enzo runs).

First step next: 2C-3 S4 (`config_migrate.rs`: report types, `migrate_locked`, D-D).

Older note: First step next: 2C-3 (see the Phase B table), in a new lead session: this
lead's context passed 55%. Older note follows. First step next: Phase B cleanup step CL3 (earlier note: CL1 Paolo writes; reviewers by kind:
Lucia for tests and Rust logic, plus Giulia or Enzo), then CL2 (Win32 detail:
Enzo plus Giulia), then CL3. Hand-over at 2026-11-19 ~10:05 UTC: lead context
46%, Claude 5-hour 31 / weekly 28, Codex 5-hour 4 / weekly 29.

Budget record for 2A-3: Claude 5-hour window 59 → 84 (lead turns grew costly as the
context grew), Codex 40 → 56.
Large specs made Codex stop halfway; three narrow steps of one concern each
finished in one pass. Prefer narrow Codex steps from the start.

Budget record for 2A-2: Claude 5-hour window 40 → 56, Codex 22 → 39 (three
implementation runs, three Codex reviews, two Sonnet reviews, four correction
rounds). The closing workflow alone used about 7 Claude points (Haiku runners
plus Sonnet reviews) and 5 Codex points. Codex should get the first try with
a narrower spec; rename and Win32 struct work needed a second round.

Budget note: one Claude implementer, gate and reviewer per round used about
40 points of the 5-hour Claude window in half an hour, while three Codex
reviews used 2 points of Codex's. Later runs should let Codex implement, keep
Claude to a single review per round, and run the gate as a plain command.
