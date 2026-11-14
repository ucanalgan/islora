# Feature-owned presentation (Step 1)

Presentation preferences are interpreted by `island-core::config` and passed
by the application to the native compositor and scenes. They never enter the
activity scheduler. `IndicatorStyle` owns system-level geometry and painting;
`MediaLayout` owns media information and compact fitting. The compositor uses
the same indicator geometry as the painter and includes media layout and artist
in the compact fitting key when needed.

## Former density dependencies

| Dependency | Resolution |
| --- | --- |
| Level layout, indicator chips, painter, window region | IndicatorStyle: Ring, Edge, Context |
| Battery alerts with a confirmed percentage | Deliberately use the same indicator style; other alerts retain their event-specific alert presentation |
| Compact media fit, title and artist | MediaLayout; compact hides artist, standard and informative show it when present |
| Expanded media | Existing artwork, controls and progress remain; informative additionally shows confirmed playback state |
| Timer | Fixed header, ring and remaining time; removed the redundant percent label (the ring already conveys it) |
| Home | Fixed date and existing controls |
| Other alerts | Fixed existing balanced artwork size |
| Quick Controls | Fixed layout retains available calendar location and confirmed level values |
| Settings cards, app state, scene and compositor | Separate feature selections and typed preferences; global density removed |
| Configuration | Legacy value read only at the config boundary, never rewritten automatically |

The media snapshot has no album name or source application field. Informative
therefore uses existing artist, playback state, and timing only; it does not
invent metadata. Media layout does not change the island's compact/expanded
state or the playback controls. The sound wave and companion spacing remain
governed by the existing rules.

## Temporary configuration compatibility

`ui_density` maps minimal to ring/compact, balanced to edge/standard, and
detailed to context/informative. Missing or invalid legacy values use
edge/standard. Valid `indicator_style` and `media_layout` override their own
legacy fallback independently. The settings UI writes only the selected new
key; existing lines and backup/recovery are retained. Formal one-time migration,
including removal of the obsolete legacy key, belongs to Step 3.
