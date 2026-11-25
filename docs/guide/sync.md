# OSM Upload

> Downloading and uploading are two independent features. For downloading see [Quick Start](quick-start.md#2-download-osm-data-optional) or the toolbar's cloud download icon (bbox selection / Overpass download window).

## Prerequisite: logging in

Complete OAuth login in **Help → Settings… → OSM Account** (see [Settings and Languages](settings-and-i18n.md)).

## Uploading local changes

1. **File → Upload…**
2. Fill in a changeset comment
3. Click "Upload local changes", or "Upload selection only" (new-object dependencies are included automatically)

Flow: `create changeset` → `POST /changeset/{id}/upload` (osmChange) → `close changeset`.
On success the panel shows the number of objects confirmed by the server.

## 409 conflicts

When upload returns 409, the upload window lists the conflicting objects (type/id/version) and you can handle them one by one:

- **Keep local and retry (KeepLocal)**: align the local object version with the server version and re-upload (overwriting remote changes)
- **Take remote (TakeRemote)**: fetch the server version of that object and replace the local changes
- **Merge tags / Merge geometry**: merge remote and local changes by tags or by geometry

Manual merging (selection by field/geometry) is still to come; `neo-editor::MergeCase` and three-way tag merging are already in place.
