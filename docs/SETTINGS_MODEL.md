# Feature settings and defaults

`island_core::settings::Settings` reads the existing line-oriented config file into feature-owned optional overrides. Invalid values are treated as absent; the file is not rewritten on read. Accessors resolve fixed defaults. The existing `ui_density` compatibility fallback is resolved independently for indicators and music, and remains in the file for the later migration. Existing files created by earlier versions may already contain explicit default-valued keys; those are retained as choices.

`RuntimeContext` and `Effective` expose the effective wave and material plus a reason when a temporary constraint applies. Energy Saver, transparency, and motion constraints never write to preferences. Other feature preferences stay independent of these decisions.

`reset_feature` and `reset_all` produce updated file contents, retaining unknown keys, comments, and the calendar URL. They remove all occurrences of owned keys without writing defaults. Until Step 3 migrates legacy density, it remains a fallback after a presentation override is reset; this preserves compatibility without recording a new user choice. These are core reset operations, not Settings window actions: the caller must apply changes to the running app and external state (including Windows autostart), then save using the existing backup/replace path and report errors. Calendar credentials deliberately require their existing separate removal action.

Settings are written to the same `config.toml`, with a synced draft and backup. Existing file reads no longer append default-valued settings: this keeps absent overrides absent after reset. The existing reload command applies supported preferences without restarting. Calendar refresh occurs only after its URL is saved.

## Ownership and fixed defaults

| Owner | Keys | Default or fallback |
| --- | --- | --- |
| Appearance | `island_style`, `material`, `reduced_motion` | island, black, off |
| System Indicators | `indicator_style` | edge, or legacy density |
| Music Player | `media_layout`, `sound_wave`, `wave_style` | standard or legacy density, on, normal |
| Activities | `task_layout`, `progress_style`, `timer_presets`, `start_mode` | bubble, ring, 5/10/15/25 minutes, focus |
| System & Notifications | `notices_off`, `windows_notifications`, `focus_hides_notifications`, `muted_apps` | empty, off, on, empty |
| Quick Controls | `quick_controls_timeout` | 8 seconds; zero disables automatic collapse |
| General | `autostart`, `monitor` | startup enabled on a new install; system monitor selection |

Absent and invalid overrides resolve independently. A valid new presentation key wins over its own legacy fallback: minimal means ring/compact, balanced means edge/standard, and detailed means context/informative. Old generated values cannot be distinguished from deliberate choices. No migration or rewriting of `ui_density` occurs here.

The wave style availability reports its parent-disabled reason independently of motion constraints. Stored child choices survive disabling the wave. Energy Saver and transparency can constrain material, and Energy Saver or reduced motion can constrain wave behavior without changing the file. Only runtime signals already available to the app are used.

Quick Controls retains its existing entry, panel navigation, keyboard and focus handling, timer and calendar pages, Windows control dispatch, and interaction-driven timeout. Its timeout is independently owned and resetting it does not reset the timer or calendar. The calendar URL is personal integration data, excluded from feature and all-settings resets, as are unknown keys. Reset functions prepare text only; future UI must coordinate applying and saving it.

## Application and persistence audit

| Owner and mutation | Runtime application and save failure |
| --- | --- |
| Indicators and music: indicator style, media layout | App preferences and renderer update together; both are restored if saving fails. |
| Appearance: material, island shape, reduced motion | Renderer or placement is applied first; rollback is attempted and failure is reported if the renderer or window cannot be restored. |
| Music: wave switch and wave style | Renderer is applied first; rollback is attempted, including the preference and effective motion. Explicit wave disable takes precedence over reduced motion. |
| Activities: task layout, transfer progress, timer presets and start mode | Island state and corresponding app state are restored on failure. Home, Quick Controls and Settings use the same start-mode path; an open Settings window is synchronized. |
| Quick Controls: timeout | The previous in-memory timeout is restored. Zero continues to disable collapse. |
| Notifications: notice filters, focus suppression, muted apps | Filters and notification queues are restored on failure. Muting from a notification also keeps the displayed notification if saving fails. |
| Notifications: Windows access | A failed access request is not saved. An already-issued Windows consent prompt cannot be cancelled; a later save failure stops the source but warns that external effects may remain. Disabling retains the source until save succeeds. |
| General: monitor | Placement is attempted first; the previous selection and placement are attempted on failure. Windows display changes and renderer failures may prevent restoration. |
| General: autostart | Registry writes are checked and the exact previous entry is restored on save failure. Registry errors can prevent rollback; the Settings display reads the actual registration. |
| Calendar integration | Validation and save occur before refresh; failure retains the previous saved URL. Credentials remain excluded from reset. |

The shared save helper reports whether rollback succeeded; it does not claim a complete rollback if a platform operation fails. Notification consent and other asynchronous Windows effects cannot be made atomic with the file. On failed rollback a session-only or external change can remain and the user is told to check the setting. Runtime context changes never invoke persistence.

The writer syncs a draft, creates a synced backup copy, then replaces the primary file using the platform's rename behavior. Interrupted-write recovery uses the backup when the primary is absent. This is not a guarantee against every power-loss or filesystem failure. Tests cover draft/backup failures without changing the primary, missing paths, recovery, save and rollback outcomes, shared start-mode logic, and wave constraint precedence. The tests do not exercise actual Windows UI, registry, display devices, consent, or every low-level write/sync failure; these require manual verification or additional fault injection.

Step 3 owns migration; Step 4 owns reset controls and confirmations; future media and wave designs remain separate work.
