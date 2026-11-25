# Editing Operations

## Keyboard shortcuts

| Key | Function |
|---|---|
| `S` / `A` / `X` / `M` | Select / Draw / Delete / Measure |
| `Enter` | Finish the line currently being drawn |
| `Delete` | Delete the selection |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo |
| `Ctrl+S` | Save XML |
| Left click | Select (red highlight); `Ctrl` appends |
| Left drag | Select and move nodes/ways |
| Right drag | Pan the map |
| Wheel | Zoom at the mouse position |

> Shortcuts do not fire while an input field has focus (to avoid accidental actions while editing tags).
> Current bindings can be viewed in **Help → Settings…**.

## Selection and movement

- Nodes/ways have a **16 px selectable margin**; the two are independent and do not steal from each other.
- Left-dragging a hit feature **auto-selects it first** and then starts dragging; the feature follows directly during the drag, and on release one history entry is recorded.
- An already selected set can be dragged from any position; when no feature is hit, the pointer must be within the 16 px margin of a selected feature.
- Hidden nodes cannot be selected when clicking/hovering; ways below hidden nodes are still clickable. **Box selection is pure geometry**: nodes not shown at the current zoom LOD are selected if they fall inside the box.
- When a way is selected or hovered (requires zoom ≥ 17), all its nodes are displayed in the normal node style and are selectable; pressing the small triangle at a segment midpoint inserts and drags a node.
- Tags do not allow empty values: after you clear a tag's value and leave the edit box, the tag is deleted immediately.

## Drawing

1. Press `A` or click the pencil icon (point / line / area tool)
2. Click the map in sequence to add nodes (a way is created automatically); `Space` is equivalent to a left-click point
3. Line/area: `Enter` finishes (at least 2 nodes); after finishing, the new feature is selected automatically and the **Select** tool is restored. The point tool likewise selects the point and returns to Select after placing it
4. Right-click while drawing: undo the previous drawing step; if the previous step was not drawing, nothing is undone
5. A new area's built-in `area=yes` is cleared first when a preset is applied, then the preset tags are written (if the preset contains `area=yes`, it is added back)

### Snapping and continuing drawing

- While drawing, the pointer snaps to nearby nodes or segment projections (default radius 16 px, adjustable in **Settings → Editing**).
- Snap feedback is shown only on the canvas (highlighted nodes/segments, rubber band pinned to the snap point); the status bar does not indicate it.
- Hold `Alt` to temporarily disable snapping.
- **Line tool**, when the draft is empty: clicking an endpoint of an unclosed way loads that way and continues appending from that end (clicking the first end reverses the node order first).
- When snapping to a segment, the placed point creates a new shared node and inserts it into that way (for point / line / area tools alike).

## Path operations

| Operation | Condition |
|---|---|
| Split at point (scissors tool) | Select "Split path at point" in the left toolbar/tool menu, then click an **internal node** on a path to split all paths passing through it (one undo) |
| Reverse direction | Select a path; the node order is reversed |
| Merge end-to-end paths (merge arrow) | Select two or more paths whose **endpoints are adjacent**; the whole chain is merged into one, directions handled automatically, one undo |
| Simplify | Select a path; RDP simplification with the current zoom tolerance |
| Delete | Delete the selected objects; referenced nodes are protected |

The tool menu is grouped by category: select/draw, measure, path (split↔merge, reverse, simplify), node (glue↔unglue, align, distribute, mirror), area and other geometry.

## Presets and validation

- After selecting an object, the "Presets…" search at the top of the tag panel searches **id-tagging-schema**. After typing a name, brand suggestions from **name-suggestion-index** also appear (marked "Brand"). Selecting one writes the tags and removes stale `*:wikipedia`.
  Then click "Apply" to commit;
- "Validation Results → Run Validation" on the right checks the 24 built-in rules; clicking a problem locates the object.

## Tag editing

The "Tags / Members" table on the right:

- Edit `key` / `value` directly; changes take effect on blur (clearing the value and blurring = delete the tag)
- `+ Add` adds a row; `×` at the end of a row deletes it
- When multiple objects are selected, the tags of all selected objects are shown as a **union**. If a tag is missing from some objects or values disagree, the value column shows "…"; click to pick an existing value from the dropdown, and the chosen value is written to all selected objects
- One edit (including multi-select) is committed through the same transaction; `Ctrl+Z` undoes it in one step

## Undo and history

- All data modifications go through Command/Transaction and are reversible with `Ctrl+Z`/`Ctrl+Y`.
- The undo stack keeps all operations in the session by default; after the program is closed, they are recovered from the journal, see
  [Files, Sessions and Recovery](files-and-sessions.md).

## Read-only layers

After unchecking a layer's "Active" switch, the data is shown in grayscale at 50% opacity and cannot be edited; check it again to continue editing.
With multiple layers, only the active layer is editable.

## Measuring

Press `M`, then click the start and end points; a yellow line and distance (m/km) are shown; click again to start a new measurement.
Measuring does not write data or enter history.

![Measure tool](../assets/ui-measure.png)
