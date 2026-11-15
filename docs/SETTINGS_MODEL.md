# Feature settings and defaults

`island_core::settings::Settings` reads the existing line-oriented config file into feature-owned optional overrides. Invalid values are treated as absent; the file is not rewritten on read. Accessors resolve fixed defaults. The existing `ui_density` compatibility fallback is resolved independently for indicators and music, and remains in the file for the later migration. Existing files created by earlier versions may already contain explicit default-valued keys; those are retained as choices.

`RuntimeContext` and `Effective` expose the effective wave and material plus a reason when a temporary constraint applies. Energy Saver, transparency, and motion constraints never write to preferences. Other feature preferences stay independent of these decisions.

`reset_feature` and `reset_all` produce updated file contents, retaining unknown keys, comments, and the calendar URL. They remove all occurrences of owned keys without writing defaults. Until Step 3 migrates legacy density, it remains a fallback after a presentation override is reset; this preserves compatibility without recording a new user choice. These are core reset operations, not Settings window actions: the caller must apply changes to the running app and external state (including Windows autostart), then save using the existing backup/replace path and report errors. Calendar credentials deliberately require their existing separate removal action.

Settings are written to the same `config.toml`, with a synced draft and backup. Existing file reads no longer append default-valued settings: this keeps absent overrides absent after reset. The existing reload command applies supported preferences without restarting. A failed save is reported and the in-memory selection may remain active until the next reload or restart. Calendar refresh occurs only after its URL is saved.

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

Settings handlers apply runtime changes before writing where supported. Material, island shape, wave style, the wave switch, and reduced motion attempt to restore the preceding runtime state if saving fails; other existing handlers can leave a session-only change after a failed save and report the error. External Windows actions are not guaranteed reversible. Backups and recovery preserve the last complete file; failed reads do not normalize it. Live reload is available through the existing command. Step 3 owns migration; Step 4 owns reset controls and confirmations; future media and wave designs remain separate work. Automated tests do not establish real-device behavior; Windows controls, display/DPI, autostart, and interruption recovery need manual verification on a Windows device.
