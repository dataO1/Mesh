# Importing Denon Engine playlists into mesh

**Status:** researched and validated. **Deferred — see TODO below.** The 2026-07-30 import
went ahead as a single flat `Illius` playlist, which needs none of the changes described here.
**Date:** 2026-07-30
**Context:** A friend's Denon Engine USB stick (945 tracks, 195 playlists) was copied to
`~/Music/Illius/` and separated into stems for mesh import. The playlist tree was exported to
`~/Music/Illius/_playlists/`. This document records how the playlists could be pushed into
mesh's DB automatically, and what has to be true for that to be safe.

Everything below marked **verified** was checked against the actual Engine DB, the actual
mesh source, and the live `mesh.db`. Everything marked **assumption** was not.

---

## 1. The join key (verified)

This is the load-bearing fact: mesh already stores the original filename, so Engine playlist
entries can be resolved to mesh track IDs without fuzzy matching.

```
Engine  Track.filename           "Mefjus - Blitz-1.mp3"
                ↓ strip extension
audio-separator output           "Mefjus - Blitz-1_(Vocals)_htdemucs_ft.wav"
                ↓ parse_stem_filename()   crates/mesh-cue/src/batch_import.rs:420
mesh    base_name                "Mefjus - Blitz-1"
                ↓ batch_import.rs:843     track.original_name = base_name
mesh    tracks.original_name     "Mefjus - Blitz-1"
```

So the join is exact string equality:

```
Engine.Track.filename minus extension  ==  mesh.tracks.original_name
```

**Verified:**

- `original_name: String default ''` is a real persisted column (`db/schema.rs:457`), and it is
  in `TRACK_COLUMNS` (`db/queries.rs:11`), so `get_all_tracks()` returns it populated.
- All **942/942** Engine `Track.filename` values match a file on disk in `~/Music/Illius/`
  exactly — zero misses, checked with `comm` over sorted name lists.
- All **945** on-disk basenames are unique, so `original_name` is a unique key for this corpus.
  No disambiguation logic needed.
- 3 files exist on disk but not in the Engine DB (`Dimension - Offender`, `Sub Focus - Siren`,
  `Sub Focus - Solar System`). They import fine, they just belong to no playlist.

**Caveat (verified in code, not yet measured on live data):** `original_name` was added by a
schema migration (`db/schema.rs:659-714`) that back-fills existing rows with `''`. Any track
imported before that migration has an empty `original_name` and will not join. This does not
affect the Illius tracks (freshly imported, so populated), but a general-purpose importer must
handle empty `original_name` and fall back to `path` basename or title/artist.

---

## 2. Target schema (verified)

```
playlists        { id: Int => parent_id: Int?, name: String, sort_order: Int }
playlist_tracks  { playlist_id: Int, track_id: Int => sort_order: Int }
```
`db/schema.rs:474-493`.

Existing API in `db/queries.rs`, all reachable via `DatabaseService`:

| Function | Line | Notes |
|---|---|---|
| `Playlists::create(db, name, parent_id) -> i64` | 497 | **ID is a millisecond timestamp — see §4** |
| `Playlists::get_by_name(db, name, parent_id)` | 542 | `parent_id: None` matches `is_null(parent_id)` only |
| `Playlists::add_track(db, pl, track, sort_order)` | 616 | one row per call |
| `Playlists::add_tracks_batch(db, pl, &[(track_id, sort_order)])` | 634 | single `:put`, use this |

Both writes use CozoScript `:put`, which is an **upsert, not a rewrite**. Inserting a playlist
touches only the rows it names. This is what makes the whole operation non-destructive:
re-running an import updates its own rows and leaves every other playlist alone.

---

## 3. Access path (verified)

mesh.db is **CozoDB on the sqlite backend** (`db/mod.rs:60`). On disk that is a single opaque
table:

```sql
CREATE TABLE cozo (k BLOB primary key, v BLOB);
```

Consequences, both verified:

- **No external tool can read or write it.** Not `sqlite3`, not a Python script. Confirmed
  `pycozo` is not in nixpkgs. Every access must go through the Cozo engine.
- Therefore the importer **must be a Rust binary linking `mesh-core`**. There is existing
  precedent for exactly this shape: `crates/mesh-core/src/bin/db_inspect.rs` and
  `crates/mesh-cue/src/bin/dump_track_list.rs` both open a collection via
  `DatabaseService::new(&collection_root)` and run ad-hoc CozoScript through `db.db().inner()`.

There is **no existing M3U or playlist import** anywhere in mesh (grepped `mesh-cue` and
`mesh-core`; nothing). This would be new surface.

**Locking:** sqlite is single-writer. The importer must not run while `mesh-cue` or
`mesh-player` has the collection open. This is the same DB-lock race that motivates using the
`reanalyze_ml` CLI over the GUI for re-analysis.

---

## 4. The one real hazard: playlist ID generation

`Playlists::create` (`db/queries.rs:497-522`) mints IDs like this:

```rust
let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
```

There is no uniqueness check, no retry, and no collision handling — and the write is `:put`,
which silently overwrites. **Two playlists created in the same millisecond produce one playlist,
and the first one is destroyed.**

For hand-creation in the GUI this never fires. For bulk-creating 195 playlists in a loop it is a
genuine risk, and it is not hypothetical for mesh's own code either: `export/service.rs:202-213`
creates playlists in a tight `for` loop with no delay during USB export.

Whether it actually collides depends on whether one sqlite commit reliably takes >1 ms. That is
**an assumption I did not measure.** Do not rely on it.

**Constraint: the fix must stay inside the core library.** Writing the `playlists` relation
directly with hand-assigned IDs would bypass the schema-migration path and the service-layer
invariants, so it is off the table. Everything below uses only `DatabaseService` methods.

The hazard is fully avoidable within that constraint, because `create_playlist` **returns the ID
it minted** and that ID is exactly `now_ms`:

1. **Prevent** — before each `create_playlist`, spin until the wall clock has advanced past the
   last minted ID. Since the ID *is* the clock, a strictly-greater clock guarantees a fresh ID.
   Costs ≤1 ms per playlist (~200 ms for all 195).
2. **Detect** — keep a `HashSet<i64>` of every ID seen (pre-existing ones enumerated via
   `get_root_playlists` / `get_child_playlists`, plus each newly minted one). Assert the returned
   ID is not already in the set. With step 1 this never fires; if it ever does, abort immediately
   rather than continue over clobbered data. This check is what the current code lacks.

**Idempotency**, also within the API: call `get_playlist_by_name(name, parent_id)` first and
reuse the existing playlist if present, creating only when absent. Without this, importing twice
creates 195 duplicates, because `create_playlist` never checks for an existing name.
`add_tracks_to_playlist_batch` is keyed on `(playlist_id, track_id)`, so re-running upserts
`sort_order` in place instead of appending.

**This is a latent bug in mesh independent of this import** and is worth fixing at the source.

---

## 5. Hierarchy and the USB export constraint (verified)

The Engine DB holds **302 playlists**, of which **197 have at least one track present on the
stick**: 13 roots + 184 children. The populated subset is exactly **2 levels deep**. The DB does
contain 76 depth-3 playlists, but all of them are empty (under `Imported Playlists`), so they
carry nothing. `playlists.parent_id` maps onto the populated 2-level shape directly.

That matters because **USB export cannot handle more than 2 levels.**
`export/service.rs:203-208` resolves a child's parent by name:

```rust
staging_db.get_playlist_by_name(pname, None)   // parent_id = None
```

`get_by_name` with `None` matches `is_null(parent_id)` (`db/queries.rs:555-560`) — i.e. **it only
ever finds root-level playlists.** A 3-level tree silently loses its third level on export: the
grandchild is created with `parent_id = None` and surfaces as a root playlist on the player.

**This is not hypothetical for the intended layout.** Nesting the import under a single `Illius/`
root produces `Illius / DnB / # Neuro` — 3 levels. On export, placing `# Neuro` under `DnB` looks
for `DnB` among *roots*, but `DnB` is now a child of `Illius`. The lookup fails and all 184
children land at the stick's top level. Unlike the ID collision in §4, which is probabilistic,
this failure is **guaranteed** by adding the extra root.

### Verified empirically, 2026-07-30

Two throwaway tests were added to `usb/sync.rs`, run, and reverted (file is clean again).
Baseline before and after: **22 `usb::` tests pass, 0 fail.**

**Defect 1 — root-only parent lookup.** Built `Illius / DnB / # Neuro` in an in-memory DB and
performed the exact lookup export does:

```
root-only lookup of 'DnB' -> None
scoped   lookup of 'DnB' -> Some(1785445109702)
```

The parent exists, but export's `get_playlist_by_name("DnB", None)` cannot see it. `# Neuro`
would be created with `parent_id = None`, i.e. as a root. **Flattening confirmed.**

**Defect 2 — the topological sort is 2-level.** `build_sync_plan` on the same 3-level tree
produced creation order:

```
["Illius", "# Neuro", "DnB"]
```

The grandchild is scheduled **before its own parent exists**. So fixing the lookup alone is not
sufficient; ordering must be fixed too. The comparator (`sync.rs:795-801`) only separates
`parent_name == None` from `Some(_)`, and every `sort_order` is 0 because `create()` hardcodes it.

**Why this was never hit before:** at 2 levels a child's parent *is* a root, so the root-only
lookup succeeds and roots-then-children ordering is sufficient. The existing tests
(`test_build_sync_plan_nested_playlists`) encode exactly that 2-level shape. The code is correct
by construction for ≤2 levels and breaks only at 3+.

**Correction:** `resolve_playlist_path` cannot be used directly here, contrary to an earlier note
in this document. It takes a full `playlists/a/b/c` path, but `PlaylistExportInfo.parent_name`
carries only a **leaf** name — the data model never had the path. The fix is instead to keep a
`HashMap<name, id>` of playlists as the export loop creates them and resolve parents from it.

### The live collection has no nested playlists at all (verified)

Opened a copy of `~/Music/mesh-collection/mesh.db` read-only:

```
TOTAL playlists: 12
depth histogram: {1: 12}
```

All 12 are roots. **The `parent_name = Some(_)` branch has never executed against this data.**
That is the complete answer to why the defect was never observed — not merely "no 3-level trees",
but no nested playlists whatsoever.

### The correct fix

An earlier draft of this document proposed resolving parents from a `HashMap<name, id>` built
during the export loop. **That is wrong and is retracted.** It works for the current import only
because leaf names happen to be unique across its 72 playlists. In general leaf names collide —
`DnB / # Neuro` and `DnB Justus / # Neuro` — and a name-keyed map would silently attach children
to the wrong parent.

The root cause is the data model: `PlaylistExportInfo.parent_name` holds a **leaf name where it
needs a path**. The codebase already uses qualified paths for the analogous case —
`PlaylistTrack.playlist` is `"Live Sets/Opening"`, and `build_qualified_name()` (`sync.rs:164`)
exists with a test asserting `"Live Sets/Opening/Warm Up"`. `PlaylistExportInfo` is the outlier.

Fix, in three parts:

1. `parent_name` carries the parent's **qualified path**, computed with the existing
   `build_qualified_name`, on both the local (`sync.rs:256`) and USB (`sync.rs:450`) sides.
2. `sync.rs:795-801` comparator orders by depth (separator count; `None` = 0) instead of
   root/non-root.
3. `export/service.rs` resolves the parent by qualified path — either via
   `resolve_playlist_path` or a `HashMap<qualified_path, id>` filled as playlists are created.
   Apply symmetrically to the `playlists_to_delete` loop (`service.rs:438`), which has the same
   root-only lookup.

### Risk

An earlier draft warned "never touch `PlaylistExportInfo`, it is the `Hash`/`Eq` diff identity."
The premise is right but the conclusion was too conservative, and is corrected here:

For any tree of depth ≤2, a child's parent is a root, and **a root's qualified name is identical
to its leaf name**. So the new `parent_name` value equals the old one for every tree that can
exist under the current code, the `PlaylistExportInfo` identity is unchanged, and existing sticks
diff exactly as before. No mass delete/recreate.

Combined with the measurement above — the live collection is 100% depth-1 — there is nothing in
the existing data that this change can alter. Regression baseline: 22 `usb::` tests pass.

**Unrelated existing hazard worth knowing before any export:** `local.playlists` is built only

**Unrelated existing hazard worth knowing before any export:** `local.playlists` is built only
from *selected* playlists (`sync.rs:246-249`, descendants pulled in via `collect_descendants`),
while `usb.playlists` is everything on the stick. Playlists present on the USB but absent from
the current selection are scheduled for deletion. This is pre-existing sync semantics, not
introduced by the fix, but it is the realistic way to lose playlists on a working stick.

Also note `create()` hardcodes `sort_order: 0` for every playlist, so playlist *ordering* within
the tree is not settable through the existing API — only track ordering within a playlist is.

**Player side:** `usb/storage.rs` browsing is playlist-only ("Track audio files are stored in
tracks/ but browsing is playlist-only", `usb/storage.rs:4`), and it respects `parent_id` for
nested display. So playlists are the *only* way tracks are reachable on the player — which makes
getting this right the difference between a usable stick and an unusable one.

---

## 6. Proposed shape (not implemented)

A new bin, e.g. `crates/mesh-cue/src/bin/import_playlists.rs`, following the `dump_track_list`
pattern:

```
import_playlists --collection ~/Music/mesh-collection \
                 --csv ~/Music/Illius/_playlists/all-playlists.csv \
                 [--prefix "Illius/"] [--only "DnB / # Neuro"] [--dry-run]
```

Steps:

1. Open the collection read-only first; build `original_name -> track_id` from `get_all_tracks()`.
2. Parse the CSV (`playlist, position, artist, title, album, genre, bpm, filename`). Strip the
   extension from `filename` to get the join key. `position` is already true Engine order.
3. Report the match rate and **exit** if `--dry-run`. Anything below ~100% for this corpus means
   something upstream changed and should be investigated, not worked around.
4. Create playlists parent-first via `create_playlist`, with the prevent + detect guards from §4.
   Optionally namespace under a single root (`--prefix`) so the friend's 195 playlists don't
   flood the top level alongside your own.
5. `add_tracks_to_playlist_batch` per playlist, `sort_order = position`.
6. Re-open and verify: playlist count, per-playlist track counts, and no pre-existing playlist
   mutated.

Uses only `DatabaseService`: `get_all_tracks`, `get_root_playlists`, `get_child_playlists`,
`get_playlist_by_name`, `create_playlist`, `add_tracks_to_playlist_batch`. No raw CozoScript, so
schema migrations and service-layer invariants stay in force.

**Idempotency:** name-based lookup before create (§4) makes re-running converge instead of
duplicating. Worth having, because the first run will almost certainly want a different
`--prefix` or subset.

**Safety:** back up `mesh.db` before the first real run. It is ~25 MB; there is already a
`mesh.db.bk` from May. `:put` is non-destructive to unrelated rows, but the ID-collision hazard
in §4 means "non-destructive" is only true once the guards are in place.

---

## TODO — deferred hierarchy work

Decided 2026-07-30 not to do any of this now. The Illius import went in as one flat `Illius`
playlist (depth 1), which the existing export path already handles correctly. Everything below
is only needed if nested playlists are wanted later.

- [ ] **Playlist importer binary** (§6) — `import_playlists --csv <file>`, library-only, with the
      create guards from §4. Driven entirely by the CSV's `playlist` path column.
- [ ] **Qualified paths in `PlaylistExportInfo`** (§5) — required before any tree deeper than
      2 levels can survive USB export. Three edits: `sync.rs:256`, `sync.rs:450`, and the
      comparator at `sync.rs:795-801`, plus path-based parent resolution in `export/service.rs`
      for both the create (`202-213`) and delete (`437-443`) loops.
- [ ] **Fix `Playlists::create` ID minting at source** (§4) — the ms-timestamp collision affects
      USB export today, independently of any import work. Worth fixing on its own merits.
- [ ] **Decide the depth-3 question** — whether nested import is wanted at all, or whether flat
      playlists plus mesh's own tagging/suggestions are the better fit.

**Scanned 2026-07-30 — this work does not touch multi-stick support.** `PlaylistExportInfo` is
referenced only in `usb/sync.rs` (13 sites) and re-exported once from `usb/mod.rs`;
`build_qualified_name` only in `usb/sync.rs` (7 sites). Multi-stick playback lives in
`mesh-player/src/ui/collection_browser.rs` (`usb_storages: Vec<(PathBuf, UsbStorage)>`, one
storage per device keyed by device path), and each `UsbPlaylistStorage` builds its node tree by
calling `PlaylistQuery::get_roots`/`get_children` directly against that stick's own `mesh.db`
(`usb/storage.rs:119-190`). It never constructs or consumes `PlaylistExportInfo`. The two paths
share only the `playlists` relation. Node identity on the player is
`NodeId("{parent_prefix}:{name}")`, scoped per-device, so names cannot collide across sticks.

## 7. Open questions

- Does the 909-track live collection have `original_name` populated, or is it mostly `''` from
  the migration? Needs an ad-hoc Cozo query — which needs a Rust bin, which is why it is still
  open. Only matters if the importer is ever pointed at pre-existing tracks.
- Should the friend's playlists be namespaced under one root, or merged into the existing tree?
  Product decision, not technical.
- Is the ms-timestamp ID collision worth fixing in `Playlists::create` itself (affecting USB
  export too) rather than worked around in the importer? Recommend yes.

---

## Source data reference

| Path | Contents |
|---|---|
| `~/Music/Illius/` | 945 original MP3/WAV, flat, unique basenames |
| `~/Music/Illius/_playlists/all-playlists.csv` | 16,113 rows: playlist, position, artist, title, album, genre, bpm, filename |
| `~/Music/Illius/_playlists/m3u/` | 192 M3U files, one per playlist, absolute local paths |
| `~/Music/Illius/_playlists/tree.txt` | rendered tree of the 197 populated playlists with track counts |
| `~/Music/Illius/_playlists/illius-import.csv` | **the actual import input** — 9,898 rows, 72 playlists, 942 tracks |

`illius-import.csv` is `all-playlists.csv` filtered to the `DnB` and `Mixes` roots and re-rooted
under `Illius/`. Selection and layout live in the **data**, not in the importer: the `playlist`
column already holds the final mesh path (`Illius / DnB / # Neuro`), so the binary just creates
whatever paths it is given. Pointing it at a different CSV imports a different collection with no
code change.

Two consequences for the importer:

- It must **create every path prefix**, including nodes that own no track rows. Here that is only
  `Illius` (both `Illius / DnB` and `Illius / Mixes` do have direct rows), but the general case
  needs the walk.
- This layout is **3 levels deep**, so it depends on the §5 export fix. Without it the local DB is
  correct and the exported stick flattens to 72 root playlists.
| `~/Music/Illius/_run/separate.sh` | resumable htdemucs_ft runner |

Engine orders playlist entries as a `nextEntityId` **linked list**, not by a sort column. The
export walks it with a recursive CTE; sorting by `id` gives the wrong order. If the export is
ever regenerated, preserve that.
