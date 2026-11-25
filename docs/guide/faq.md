# FAQ

### On startup, NVIDIA shows "Press Alt+Z to open the NVIDIA overlay"

This is a prompt from the **NVIDIA App / GeForce Experience overlay** after it detects a GPU-rendering application.
It is not a NEO window and is unrelated to NEO's code. The NVIDIA overlay is a driver-level global injection, and there is
**no official switch to turn it off per application**.

How to turn it off (pick one):

1. **NVIDIA App**: Settings (gear icon) in the top right → Features → **NVIDIA Overlay** → off;
2. **GeForce Experience**: Settings → General → **In-game overlay** → off.

To disable it only while using NEO: turn the overlay off first → start NEO, then turn it back on afterwards. Reference:
[NVIDIA developer forum discussion](https://forums.developer.nvidia.com/t/how-to-prevent-in-game-overlay-from-triggering/256026).

### Boxes / tofu appear in the UI

0.1.0 bundles Noto Sans CJK SC + GNU Unifont (including Upper) as fallbacks, so this should not happen.
If it still does, take a screenshot of the "Font override examples" in **Help → About NEO** and file an issue.

### The map is blank / disappears after zooming

- The layer is hidden: check the **eye icon** in the layer panel on the right;
- Node LOD: not showing nodes at z ≤ 16 is expected behavior;
- Zooming back to a blank map was a bug fixed in 0.1.0 (scene rebuild trigger conditions + GPU upload key);
  if it still reproduces, attach the zoom level and the steps.

### A node cannot be clicked / clicking a node selects a way instead

- Nodes are not pickable when the current zoom is below the node display threshold (zoom in according to the `z` hint);
- Nodes and ways each have a 16 px margin, with nodes taking priority; hidden nodes fall back to the way below.

### Why is the layer grayed out and not editable?

The "Active" checkbox in front of the layer was unchecked: the data is displayed in grayscale at 50% opacity and is read-only.
Check it again to edit; with multiple layers, only the active layer is editable.

### Keyboard shortcuts do not respond

- Shortcuts are disabled while an input field (tag editing) has focus; click the map and try again;
- The window must be in the foreground; on Windows, if another program (such as JOSM) steals focus, keys are not sent to NEO.

### Do Chinese tags/names display correctly?

Yes. The data itself is UTF-8; the UI font chain covers CJK, and the tag table shows raw Unicode directly.

### How do I recover lost edits?

First check whether `out/neo-journal.jsonl` exists, then use **Edit → Recover from journal**.
The journal depends on the same baseline data; if the baseline file has changed, recovery may fail, but the journal is never deleted.

### Does it support uploading to the OSM API?

0.1.0 has not implemented upload yet (phase 5). You can currently export `.osc` for JOSM/other tools to upload.

### Is there a validator and plugins?

In 0.1.0, "Validation Results / Conflicts" on the right are placeholders; the plugin system (WASM) is in phase 6.

### Does it support Linux / macOS?

The code is cross-platform (wgpu + eframe), but the 0.1.0 release only provides Windows x64;
on Linux/macOS you can build from source according to the [installation documentation](installation.md).

### Data copyright

OSM data © OpenStreetMap contributors, ODbL 1.0. NEO does not distribute map data;
users who import or download data must comply with the ODbL themselves. See the repository `NOTICE` for details.
