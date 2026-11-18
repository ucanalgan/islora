# Islora 1.0 execution plan

This is the working plan for turning the repository into Islora 1.0. It is
derived from a read-only audit of the code on branch `v1/foundation`
(based on `main` at `9152ce5`), not from earlier claims in other documents.
Statuses change only when code lands with tests and an independent review.

All work happens on the local branch `v1/foundation`, one reviewed commit per
task, in dependency order. Merging, pushing, opening a pull request and any
destructive Git operation each need the user's explicit approval.

## How each task is done

The lead is Claude Opus: it plans, writes each task's spec, assigns workers,
reads their results, decides on corrections and makes the commits. Workers
are Codex CLI and Claude agents. Codex does most of the coding; a change is
always reviewed by the other kind of worker than the one that wrote it.

**The lead does no work itself.** It never reads source files, diffs, logs or
long documents and never edits code. Anything that needs reading goes to a
read-only agent that returns a short summary (a page at most); code goes to an
implementer; checking goes to a reviewer with a fresh context. The lead keeps
only plans, specs and verdicts, so its context stays small and never compacts.
A new lead session starts for each phase, from this file.

**Budget line.** Codex: stop at 45% of its 5-hour window. Claude: the user
allowed using the whole 5-hour window on 2026-11-18, so the 5-hour line is
100%; the guard is Claude's weekly window, stop at 97%. Park cleanly before
either line (see `resume-handoff`).

**Pick the worker by the job and by the limits.** Before each batch, read both
limits: Claude through the session usage tool, Codex from
`rate_limits.primary.used_percent` in the newest
`~/.codex/sessions/**/rollout-*.jsonl`. Then:

- Planning, specs and final decisions: Opus, as the lead.
- Code: Codex, or a Claude Sonnet agent when Codex has less room.
- Review: the other kind of worker than the author, Sonnet is enough for most
  diffs; Opus or Codex for security-critical ones.
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
| Existing Phase 2 work kept | done | `dbeb0d6` |
| Formatting fix for CI | done | `7114fc9` |
| Pre-2A | done: gate and presentation reader share one classifier that ignores only unrelated unsupported lines; 459 tests | see `git log` |
| 2A-1 | done: `config_fs.rs` with `ConfigDir`, `FileIdentity`, `VerifiedFile`; settings reads go through it; 471 tests | see `git log` |
| 2A-2 | done (plus review cleanup): writer lock, side-effect-free reads, `prepare_storage`, locked autostart transactions, calendar address by channel, every file operation bound to the held directory handle; 483 tests | see `git log` |
| 2A-3 | done: owner and DACL kept across replacement through exclusive drafts and pre-rename checks; 502 tests | see `git log` |
| Core state 1/3 | done: `ProtectedBackupName` (canonical build/parse), `next_sequence`, `retention_excess`, FNV-1a `ChangeFingerprint`; 507 tests | see `git log` |
| Core state 2/3 | done: `PrimaryObservation`, `ArtifactInventory`, `RuntimeMode`, `StartupOutcome`, `plan_startup` with rules a–e in precedence order; explicit 42-row matrix and precedence tests; 516 tests | see `git log` |
| Core state 3/3 | done: `FailureRecord` canonical encode/decode (no setting values, errors carry no input), `retry_decision` (corrupt record fails closed), `select_candidate` (Restore / Ambiguous / NoneValid); 521 tests | see `git log` |
| Core state cleanup | done: items above the test module (pure move), missing docs, parse and decode boundary/order tests; Sonnet: no findings | see `git log` |
| 2B-2a | next: move `replace_in`/`replace_in_with_hook` into `platform/config_commit.rs` as `commit_primary` (pure move) | — |

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

Codex never reviews these steps, because it writes them. If a major finding is
still open after two correction rounds, split the step or sharpen its spec.

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
