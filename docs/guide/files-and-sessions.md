# Files, Sessions and Recovery

## Supported formats

| Format | Read | Write | Notes |
|---|---|---|---|
| PBF (`.osm.pbf`) | ✅ | ✅ | Primary format; parallel import |
| OSM XML (`.osm` / `.xml`) | ✅ | ✅ | Can read JOSM files |
| osmChange (`.osc`) | ✅ | ✅ | Changes only; suitable for review/upload |
| JOSM session (`.jos` / `.joz`) | ✅ | ✅ | `.joz` is the zipped form |

## Opening files

- **File → Open PBF… / Open OSM XML…**
- Large imports show progress and can be cancelled; failures are shown in the status bar.
- Only one data layer is kept at a time; opening a new file replaces the current data (save first).

## Saving and exporting

- **Save XML (Ctrl+S)**: write the current data set as OSM XML
- **Export osmChange**: write only this session's creates/modifies/deletes (`create/modify/delete`)
- **Export PBF**: write the complete data set as PBF
- If there are unsaved changes before saving, the toolbar shows "Unsaved"

## Session files

**File → Save Session…** produces `.jos` (XML) or `.joz` (ZIP with data snapshot metadata):

- Preserves the viewport (center/zoom), selection, layer information, and file references
- If the file referenced by the data layer is unavailable when opening a session, an error is shown
- Sessions are not autosave; save explicitly

## Autosave (journal snapshots)

Every 60 seconds the program copies the journal to `out/autosave/neo-NNNN.jsonl` (keeping 3 generations), and also writes the journal bytes
and statistics into `out/autosave/neo-snapshots.sqlite` (keeping 10 entries, SQLite WAL). The status bar shows
"Autosaved journal snapshot #N"; if the journal is lost, startup tries to recover from the most recent SQLite snapshot.

## Journal (automatic recovery)

All edits are synchronously written to a **JSONL journal** (default `out/neo-journal.jsonl`) when committed to history:

1. Restart the program after it exits abnormally;
2. **Edit → Recover from journal**;
3. NEO replays the commands, renames the journal to `.recovered` as a backup, and opens a new journal.

Limitations:

- The journal records only **commands** and depends on the same baseline data; recovery may fail after the baseline file changes;
- A failed recovery keeps the original journal; it is never deleted.

## Data safety recommendations

- Back up the source PBF/XML before editing;
- Use `Ctrl+S` or export osmChange promptly for important edits;
- Periodically check whether `out/neo-journal.jsonl` is growing (indicating edits not yet flushed to disk).

## Environment variables (testing/operations)

| Variable | Effect |
|---|---|
| `NEO_PERF_LOG` | Write the performance log to a file |
| `NEO_SYNTH_N` | Development/performance testing only: generate synthetic data with N nodes at startup (production defaults to an empty map) |
| `NEO_START_ZOOM` | Specify the initial zoom |
| `NEO_TEST_SELECT` | Select a node/way immediately at startup (testing) |
| `NEO_LAYER_HIDDEN` | Hide the layer at startup |
| `NEO_LAYER_INACTIVE` | Start with the layer read-only/grayed out |
| `NEO_SHOW_LAYER_INFO` / `NEO_SHOW_SETTINGS` | Open the info/settings window at startup |
