# Collection Management

This guide covers how mesh organizes your music library, how to import and prepare tracks, how to export to USB for live performance, and how set recordings work.

---

## Folder Structure

When you first launch mesh-cue or mesh-player, it creates a collection at `~/Music/mesh-collection/`. On Windows, this is `~\Music\mesh-collection\`. Everything mesh needs lives inside this folder.

```
mesh-collection/
├── import/          # Drop files here for import
├── tracks/          # Stem library (8-channel FLAC files)
├── playlists/       # Playlist folders (nested)
├── presets/         # Effect presets
│   ├── stems/       # Per-stem effect presets (YAML)
│   └── decks/       # Deck-level presets (YAML, references stem presets)
├── effects/         # Pure Data effect patches and CLAP plugins
├── waveforms/       # Cached peak data for waveform display
├── mesh.db          # CozoDB database (track metadata, playlists, history)
├── config.yaml      # mesh-cue application settings
├── player-config.yaml  # mesh-player application settings
└── theme.yaml       # Color theme customization (shared between apps)
```

A few things worth noting:

- **tracks/** contains 8-channel FLAC files, not your original audio. Each file holds 4 stereo stem pairs (Vocals L/R, Drums L/R, Bass L/R, Other L/R). These are typically around 150 MB per 3-minute track.
- **mesh.db** is a CozoDB graph database. It stores all metadata, playlists, cue points, loops, play history, ML analysis results, and audio similarity vectors. You do not need to interact with it directly.
- **theme.yaml** is shared between mesh-cue and mesh-player. Both applications read from the same file, so theme changes apply everywhere.
- **config.yaml** and **player-config.yaml** are separate because the two applications have different settings (import options vs. performance options).

Do not rename or reorganize files inside `tracks/` or `playlists/` manually. The database tracks file paths, and moving files outside of mesh will break those references.

---

## Importing Tracks

Import happens in mesh-cue. Place your audio files in the `import/` folder, then open the import panel.

<!-- TODO: Screenshot -- mesh-cue import panel showing file list with progress bars and analysis status -->

### Two Import Modes

**Mixed Audio mode** is for regular audio files. This is the mode you will use most of the time. Mesh runs the Demucs neural network to separate each file into 4 stems (Vocals, Drums, Bass, Other). Processing time depends on your hardware:

| Hardware | Time per track (3 min) |
|----------|:----------------------:|
| CPU only | 2-5 minutes |
| NVIDIA GPU (CUDA) | 15-30 seconds |
| DirectML GPU (Windows) | 15-30 seconds |

You can configure the Demucs model (standard or fine-tuned), quality shifts (1-5, higher is better but slower), and whether to use GPU acceleration. These options are in the import settings.

**Stems mode** is for tracks you have already separated outside of mesh. Files must follow this naming convention:

```
Artist - Track_(Vocals).wav
Artist - Track_(Drums).wav
Artist - Track_(Bass).wav
Artist - Track_(Other).wav
```

All four stem files must be present for a track to import. The import panel shows a V/D/B/O indicator for each track so you can see which stems have been found.

### Supported Input Formats

MP3, FLAC, WAV, OGG, and M4A. Decoding is handled by the symphonia library. The internal storage format is always 8-channel FLAC (lossless).

### What Happens During Import

For every track, mesh runs the following analysis pipeline:

- **BPM detection** -- Two backends are available. Simple mode uses Essentia's RhythmExtractor for speed. Advanced mode uses the Beat This! neural network (ISMIR 2024) for accuracy, which also detects downbeats and avoids the half-tempo errors that plague fast genres like DnB and psytrance. You can choose the backend in settings.
- **Beat grid generation** -- The detected beats are refined with onset-weighted phase alignment to produce a consistent grid. This grid is what powers beat sync during performance.
- **Musical key detection** -- Via Essentia. Displayed in Camelot notation (e.g., 8A, 11B) or standard notation (Am, F), depending on your display settings.
- **Loudness measurement** -- Both integrated LUFS (whole track) and drop LUFS (loudest section). The drop LUFS is used for auto-gain during performance so that drops hit at a consistent level.
- **Audio feature extraction** -- A 16-dimensional vector representing the track's sonic character. This powers the similarity search used by smart suggestions.
- **ML analysis** -- Genre classification across 400 Discogs categories, vocal/instrumental detection (96% accuracy), and optionally mood and arousal estimation. These results are stored as tags in your library. ML analysis runs on-device using EffNet neural networks -- nothing is sent to the cloud.

<!-- TODO: GIF -- import in progress showing per-track progress bars, stem separation status, and BPM/key results appearing -->

### Parallel Import

You can configure the number of parallel import workers from 1 to 16 tracks simultaneously. More workers use more CPU and RAM. On a machine with GPU stem separation, 2-4 workers is usually a good balance since the GPU handles the heavy lifting.

Import is cancelable at any time. Progress and ETA are shown per track.

---

## Playlists

Playlists are hierarchical. You can create folders that contain other folders or playlists, and playlists contain tracks.

<!-- TODO: Screenshot -- mesh-cue dual browser panels showing playlist tree on the left and track list on the right -->

### Managing Playlists

mesh-cue has a dual browser panel layout. You can:

- Create new playlists and folders in either panel
- Drag and drop tracks between panels to add them to playlists
- Drag playlists between folders to reorganize
- Use the right panel to browse your full collection while the left panel shows a specific playlist

Playlists are stored in the database and exported to USB along with your tracks. The playlist structure on USB uses a YAML manifest (`mesh-manifest.yaml`) that describes the hierarchy.

---

## Re-analysis

If you want to re-run analysis on tracks that are already in your library, right-click in the mesh-cue browser to access the context menu.

Two re-analysis options are available:

**Re-analyse Metadata** lets you selectively re-run specific analysis steps. A dialog with checkboxes appears:

- **Name/Artist** -- Re-parse the track name from the original filename
- **Loudness** -- Re-measure LUFS levels
- **Key** -- Re-detect musical key
- **ML Tags** -- Re-run genre, mood, and vocal detection (uses the dedicated ML pipeline, not a subprocess)

**Re-analyse Beats** regenerates the BPM and beat grid from scratch. This is destructive -- it overwrites any manual beat grid edits you have made. Use it when you know the current grid is wrong and you want a fresh detection.

Both options can be applied to a single track, a selection of tracks, a playlist or folder, or your entire collection.

---

## USB Export and Sync

USB export is how you get your prepared library onto a drive for use with mesh-player on another machine or on the embedded Orange Pi standalone unit.

<!-- TODO: Screenshot -- mesh-cue export panel showing USB device selector, playlist tree with checkboxes, and sync status -->

### Export Workflow

1. Connect a USB drive. ext4 or exFAT formatting is recommended. Avoid FAT32 if possible -- its 4 GB file size limit can be a problem for very long tracks stored as 8-channel FLAC.
2. In mesh-cue, click **Export**.
3. Select which USB device to export to.
4. Mesh scans the USB for existing files to build a sync plan. If you have exported before, it detects which tracks are already present and only copies what has changed.
5. Review the sync plan. You see a hierarchical playlist tree with checkboxes. Select the playlists you want on the USB.
6. Start the export. Mesh copies stem FLAC files, the CozoDB database, presets, and playlist metadata.
7. Progress is shown per track with an ETA. You can cancel at any time.

### USB Collection Structure

The USB mirrors the local collection layout:

```
USB Root/
├── mesh-collection/
│   ├── mesh-manifest.yaml   # Playlist hierarchy
│   ├── tracks/              # Stem FLAC files
│   ├── playlists/           # Playlist metadata
│   ├── mesh.db              # Full database copy
│   └── presets/             # Effect presets
└── mesh-recordings/         # Set recordings (WAV + tracklist TXT)
```

The `mesh-manifest.yaml` file at the root of the collection is how mesh-player identifies a USB stick as containing a mesh collection.

### Sync Behavior

Export uses metadata-based change detection (file size and modification time) to avoid re-copying unchanged tracks. This means subsequent exports after the initial one are much faster.

When you perform with mesh-player using a USB collection, session history (which tracks you played, when, on which deck) is written back to the USB's database. This history persists across sessions.

---

## Database

mesh uses CozoDB, an embedded graph database, stored as `mesh.db` in your collection folder. It holds:

- Track metadata: BPM, musical key, LUFS loudness, gain adjustments, audio feature vectors
- Beat grids and downbeat positions
- Hot cues and saved loops
- Stem links between tracks
- Playlists and folder hierarchy
- Play history (session-based, with timestamps and deck assignments)
- ML analysis results: genre classifications, vocal presence scores, mood/arousal values, audio characteristics
- Audio similarity vectors for the smart suggestions engine

The database travels with your collection. When you export to USB, the full database is included. When you play on mesh-player (whether on a laptop or the embedded device), session history is written to whichever database is active -- the USB's database if you loaded from USB, or the local database otherwise.

You should not edit `mesh.db` directly. All interaction happens through mesh-cue and mesh-player.

---

## Set Recordings

mesh-player can record your master output during a live set.

### Recording Format

- **Audio**: 16-bit PCM stereo WAV, 48 kHz sample rate
- **Filename**: `YYYY-MM-DD_HH-MM.wav` (timestamp of when recording started)
- **Tracklist**: A companion TXT file with the same name, listing every track you played during the recording with timestamps and deck numbers

Example tracklist output:

```
Mesh Set Recording -- 2026-03-15 22:30
Duration: 01:45:23

00:00:00  Artist A - Track One [Deck 1]
00:04:12  Artist B - Track Two [Deck 3]
00:08:45  Artist C - Track Three [Deck 2]
...
```

The tracklist is generated automatically from session play history stored in the database.

### Where Recordings Are Saved

- **Primary**: `mesh-recordings/` on every connected USB stick that contains a mesh collection. If you have two USB sticks plugged in, both get a copy of the recording simultaneously.
- **Fallback**: If no USB stick with a mesh database is connected, recordings go to `mesh-recordings/` in your local collection folder.

### Storage Requirements

A minimum of 2 GB free space is required to start recording. At 48 kHz / 16-bit / stereo, a 2-hour set produces roughly 1.32 GB of audio.

If disk space runs out or an I/O error occurs during recording, mesh drops samples from the buffer rather than blocking the audio engine. You will not hear a glitch during your set, but the recording may have a gap.

---

## Audio File Format Details

### Internal Format

All tracks in the mesh library are stored as 8-channel FLAC. The channel layout is:

| Channels | Stem |
|:--------:|------|
| 1-2 | Vocals (L/R) |
| 3-4 | Drums (L/R) |
| 5-6 | Bass (L/R) |
| 7-8 | Other (L/R) |

FLAC is lossless, so there is no quality loss from import. The tradeoff is file size -- expect roughly 150 MB per 3-minute track at typical electronic music complexity.

### Filesystem Considerations

If you are using USB sticks formatted as FAT32, be aware of the 4 GB file size limit. Very long tracks (15+ minutes at high complexity) can approach or exceed this limit. Format your USB sticks as exFAT or ext4 instead. exFAT is the safest choice for cross-platform compatibility (Linux, Windows, macOS can all read it).

---

## Platform Differences

| Aspect | Linux | Windows | Embedded (Orange Pi) |
|--------|-------|---------|---------------------|
| Collection path | `~/Music/mesh-collection/` | `~\Music\mesh-collection\` | No local collection |
| USB detection | sysinfo + udisks2 (mount/unmount) | sysinfo | sysinfo |
| Audio backend | JACK / PipeWire | WASAPI | JACK (direct I2S) |
| GPU stem separation | CUDA (NVIDIA) | DirectML | Not available (CPU only) |
| Track source | Local or USB | Local or USB | USB only |

On the embedded Orange Pi, there is no local collection. All tracks are loaded from USB sticks. This is by design -- the device boots directly into mesh-player and expects you to bring your prepared library on a USB drive.

---

## Backup and Recovery

Your mesh collection is self-contained in the `mesh-collection/` folder. To back up your library:

1. Copy the entire `mesh-collection/` folder to your backup destination.
2. That is it. The database, tracks, presets, and configuration are all inside.

To restore, copy the folder back to `~/Music/mesh-collection/` and launch mesh.

USB exports also serve as partial backups. They contain the full database and whichever tracks you exported. If your local collection is lost, you can copy the USB's `mesh-collection/` folder back to your machine.

### What You Cannot Recover

- **Original source files** are not kept after import. Mesh stores stems, not the original MP3/FLAC/WAV. If you need the original files, keep your own copies elsewhere.
- **Manual beat grid edits** are lost if you run "Re-analyse Beats" on a track. The re-analysis replaces the grid entirely.

---

## Tips

- **Start with a small import** to check stem separation quality and BPM detection accuracy before importing your full library. If Advanced beat detection gives better results for your genre, switch to it in settings before the big import.
- **Organize playlists before exporting**. It is easier to manage a large library in mesh-cue's dual browser than it is to reorganize on the fly during a set.
- **Keep your USB sticks formatted as exFAT** unless you only use Linux, in which case ext4 is also fine. FAT32's 4 GB limit will cause problems eventually.
- **Re-analysis is non-destructive for metadata** (name, key, loudness, ML tags) but **destructive for beats**. Only re-analyze beats when you know the current grid is wrong.
- **The database is the single source of truth** for your library. If you move or rename files in `tracks/` using your file manager, mesh will not be able to find them. Always manage your library through mesh-cue.
