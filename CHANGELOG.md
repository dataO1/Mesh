# Changelog

All notable changes to Mesh are documented in this file.

---

## [0.9.13]

### Added

- **Suggestion graph view** — Both apps now show an interactive similarity
  graph of your entire library. Tracks appear as cluster-colored nodes
  positioned by spectral similarity (t-SNE). Selecting a track in the
  suggestion list highlights its node and draws an edge from the seed,
  showing where you'd "land" in the library. In mesh-cue this is a full
  interactive tab with seed navigation, intent slider, and set plan export.
  In mesh-player it's a compact read-only panel beside the track list.

- **Energy arc ribbon** — A flowing ribbon in the collection browser
  visualises set flow across three dimensions: vertical position shows
  energy/aggression, width shows spectral jump size between tracks, and
  color shows key transition quality (green = compatible, amber = moderate,
  red = difficult). Available in both mesh-cue and mesh-player.

- **Reward-based suggestion scoring** — The scoring algorithm now produces
  0-100% match scores (higher = better) with clear component breakdown:
  intensity (33%), key compatibility (30%), vector similarity (30%), co-play
  history (7%). Brute-force scoring across all tracks replaces the
  approximate nearest-neighbour search, giving every track a meaningful score.

- **Browser Analytics panel** — Energy arc and similarity graph share a
  single panel to the right of the track list. Togglable via Settings >
  Display > Browser > "Browser Analytics" (default on).

- **Blend Mode and Transition Reach settings** — New controls replace the
  old Sound Target/Focus settings. Blend Mode (Layering/Balanced/Transition)
  sets when the intent slider flips from similarity to diversity. Transition
  Reach (Tight/Medium/Open) controls how far transitions reach at the
  extremes.

- **Set plan export** — Build a set by navigating seeds in the graph view,
  then export the seed history as a playlist playable in mesh-player.

- **UMAP graph layout** — Alternative to t-SNE for the similarity graph,
  selectable via a button in the mesh-cue graph view header. UMAP preserves
  both local and global structure — similar communities appear close
  together. Toggle between t-SNE and UMAP to compare.

- **Weight tuner** — Three sliders (S/K/I) in mesh-cue's graph view let you
  tune the balance between Similarity, Key, and Intensity in the suggestion
  scoring formula. Weights auto-normalize to sum 1.0. Re-queries instantly.

- **Key filter toggle (mesh-cue)** — "Key" button in the graph view cycles
  Strict / Relaxed / Off for harmonic filtering during graph suggestions.

- **Match score column** — The suggestion list shows match percentage
  (e.g., "78%") instead of rank number. Tells you how confident the
  algorithm is in each suggestion.

- **Dynamic result count** — Suggestions are filtered to tracks above 45%
  match score instead of a fixed 30-track limit. Unusual seeds show fewer
  results; well-connected seeds show more.

### Changed

- **Intensity scoring v2** — Complete redesign of the intensity/aggression
  system. Individual audio components (spectral flux, flatness, dissonance,
  crest factor, energy variance, harmonic complexity, spectral rolloff) are
  stored per track and combined at query time with tunable weights. Replaces
  the old binary ML classifiers (mood_aggressive/mood_relaxed) which couldn't
  distinguish sub-styles within a genre. Multi-frame analysis (20 frames
  across the track) replaces single-midpoint-frame measurements. Re-analyse
  ML with tags ticked to populate the new values.

- **Percentile-rank normalization** — Vector similarity and intensity scores
  use percentile-rank normalization instead of pool-max + genre z-score.
  Guarantees uniform [0, 1] spread regardless of how narrow the raw distance
  distribution is. Fixes "everything scores 88-91%" compression.

- **Deviation-rank transition scoring** — At extreme slider positions, the
  vector component uses percentile-rank of deviation from target distance
  instead of a Gaussian bell curve. Every track gets a unique score — no
  flat zone near the peak where 160 tracks all scored 96-100%.

- **Dynamic community thresholds** — Tight/Medium/Open reach thresholds
  adapt to the library's actual cluster structure. Computed from percentile
  ranks of intra- and inter-community distances after t-SNE clustering.

- **Multi-source graph** — The similarity graph merges tracks from local
  collection and all mounted USB sticks. Deduplicates by artist-title.
  Rebuilds automatically on USB plug/unplug.

- **Smart suggestions v3** — PCA-128 similarity index with dynamic
  dimensionality (auto-detects optimal components via 95% variance
  threshold). Dual-deck context awareness seeds from the staying deck.
  Opener mode suggests candidates when no deck is loaded. Co-play bonus
  rewards tracks with proven transition history. Genre-normalized distances
  prevent tight genres from monopolizing results.

- **Deterministic graph layout** — The t-SNE graph looks consistent across
  restarts. Cluster colors are derived from spatial position (not random)
  so communities keep their colors as the library grows.

- **Suggestion tag pills** — Tags now show the raw musical relationship
  (key transition name, similarity level, energy direction) with
  theme-aware colors matching the energy arc ribbon encoding.

- **Stable track IDs** — Track identifiers use relative file paths instead
  of auto-incrementing integers. Play history, cue points, and all per-track
  data survive collection folder moves.

- **Configurable suggestion algorithm** — Settings expose Key Filter
  (Strict/Relaxed/Off), Stem Complement toggle, Playlist Split toggle,
  and Key Scoring Model (Camelot/Krumhansl).

- **Browser search** — Fuzzy artist + title matching with on-screen
  keyboard support for the embedded device.

- **OTA updates** — Live progress display with spinner, elapsed time, and
  streaming journal output.

- **Auto headphones cue** — Logarithmic volume curve for more musical
  crossfader-to-cue transitions.

### Fixed

- **Energy arc not updating on playlist switch** — Stale suggestion state
  caused the arc to render old data when switching playlists.

- **"Re-analyse Metadata" on entire collection now works** — Right-clicking
  the Collection root and choosing "Re-analyse Metadata" would silently find
  zero tracks and do nothing. Two bugs were fixed: (1) the scope resolver was
  walking the playlist tree to collect child nodes, which only returns
  sub-folder nodes like "tracks/Techno" — a flat collection where all tracks
  sit directly under "tracks/" was invisible; (2) the track display builder
  never populated the file path field that reanalysis reads, so every track
  appeared path-less even after the query found them. Both are now fixed.

- **"Build Similarity Index" now works** — The PCA index build always reported
  "0 ML embeddings found" even after a full re-analysis. The EffNet embeddings
  were being written to the database correctly, but the read-back silently
  returned empty because CozoDB's typed vector columns use a different internal
  format than the code expected. All vector reads are now fixed; building the
  similarity index works as intended.

- **Embedded device — USB sticks not detected after boot** — Plugging in a USB
  stick while mesh-player was already running on the embedded device had no
  effect; only sticks present at startup were recognised. The root cause was
  that the custom udev automount rules called `systemd-mount` from within
  udev's private mount namespace, where the mount was silently discarded.
  The automounting stack has been replaced with udiskie + udisks2 — the same
  combination used on the development laptop — so hotplug, eject, and
  re-insert all now work reliably.

- **USB export — playlists not pre-selected when stick already connected** —
  Opening the export panel with a USB stick already inserted did not
  automatically tick the local playlists that exist on the stick. The
  auto-select logic was comparing node IDs directly, but local IDs use the
  format `playlists/Name` while USB IDs use `playlist:Name`, so they never
  matched. The comparison now normalises both sides before matching.

---

## [0.9.12]

### Changed

- **Smart suggestions — dual harmonic filter** — The single fixed-threshold
  filter has been replaced by a two-layer gate. A permanent **harmonic floor**
  (`base_score ≥ 0.45`) blocks Semitone, FarStep, FarCross, and Tritone
  transitions at every intent fader position regardless of energy bias or
  personal curation — these are musically dissonant and never appropriate. A
  separate **blended threshold** (`key_transition_score ≥ 0.65`) operates on
  the energy-direction-blended score: at the centre position this equals the
  raw base score, so EnergyBoost/Cool (0.50) are excluded and only the
  flow-safe set (SameKey, Adjacent, Diagonal, MoodLift) appears — suitable for
  mashups and multi-track layering. At extreme positions the blended score shifts
  toward the energy-direction component: EnergyBoost rises to 0.75 and unlocks;
  SameKey falls to 0.50 and is filtered out. The crossover happens naturally
  near ±0.60–0.70 bias without any hard-coded knee. Personal curation (currently
  browsed playlist) still receives 50% leniency on the blended layer only —
  the harmonic floor is never relaxed.

- **Smart suggestions — static key harmony weight** — The harmonic
  compatibility weight (`w_key`) is now constant at 30% across all intent fader
  positions (previously it dropped from 25% to 10% at extremes). Key harmony is
  treated as a hard quality constraint: an energetic transition that clashes
  harmonically sounds wrong regardless of intent. BPM and key-direction weights
  instead shoulder the budget reduction as energy-related terms (aggression,
  danceability) grow at extremes. At extreme bias, BPM weight falls to zero and
  key-direction drops to 5%; at centre, BPM carries 13% and key-direction 12%.

- **Smart suggestions — spectral diversity at extreme intent** — The HNSW
  similarity component now flips direction as the intent fader moves toward
  extremes. At the centre position it rewards spectral similarity (find tracks
  that sound like the seed); at extreme positions it rewards spectral diversity
  (find tracks that complement rather than copy the mix). At the halfway point
  the component is flat for all candidates so energy signals drive the ranking
  uncontested. Distances are normalised within the candidate pool before
  blending so the effect is consistent regardless of collection size. The
  reason-tag label updates to reflect the active mode: "Similar" at centre,
  "Spectral" in the transition zone, "Variety" at extremes.

- **Smart suggestions — fixed harmonic filter** — The adaptive harmonic filter
  threshold (which relaxed from 0.50 to 0.10 at extreme intent) has been
  replaced with a fixed 0.50 threshold. The relaxation was redundant: the
  energy-direction blend inside `key_transition_score` already makes the filter
  energy-aware at extremes — energy-appropriate transitions (SemitoneUp when
  raising) naturally score above 0.50 while opposing ones (EnergyCool when
  raising) fall below. The old relaxation was flooding the pool with
  harmonically bad candidates and contributing to the uniformity problem.

- **Stem mute/unmute fades** — Toggling a stem mute no longer cuts or restores
  audio instantly. A 50 ms linear fade is applied entirely inside the engine so
  the external API is unchanged. Muting fades the stem out over 2 400 samples
  (at 48 kHz); unmuting fades it back in at the same rate. The fade is applied
  after multiband processing so effects trail off cleanly, and the per-sample
  ramp runs on both the normal playback path and the scratch path. Solo
  transitions also benefit — soloing a stem fades the others out rather than
  cutting them. The duration is a single constant (`STEM_FADE_SAMPLES`) and
  trivial to tune by ear.

- **LUFS gain compensation — perceptual density bias** — Both quiet and loud
  tracks now receive a small extra correction beyond straight linear gain. The
  issue is perceptual density: a track at −4 LUFS, cut to match a −9 LUFS
  target on the meter, still carries the spectral saturation and consistent RMS
  of a heavily limited track and will punch through a mix even at the same
  measured level. Equally, a −14 LUFS track boosted to −9 LUFS still feels
  sparse because it lacks that density. The correction uses the formula
  `gain = delta × (1 + 1/|target|)` — the bias auto-scales with the target
  level so it is stronger at a loud mixing standard (−6 LUFS, ≈ +16.7%)
  and gentler at a dynamic one (−9 LUFS, ≈ +11.1%), reflecting how perceptual
  density differences matter more when everything is loud.

### Added

- **Playlist-aware smart suggestions** — The suggestion panel now splits into
  two independent halves. The top 15 slots show the best-matching tracks from
  the playlist you are currently browsing; the bottom 15 show the best global
  matches from all other sources (other playlists, USB sticks, and the full
  local collection). Global rows are visually tinted so the split is immediately
  obvious at a glance.

- **Per-track playlist pills** — Every suggestion row now shows which playlists
  that track belongs to as blue pill tags, regardless of what you are currently
  browsing. If a suggestion appears in your "Breakbeat" and "Live Set" playlists,
  both names show on the row.

- **Deeper playlist matching** — Tracks in the browsed playlist use a more lenient
  harmonic filter (50% of the normal threshold) so that slightly less obvious key
  relationships within your own curated set are still surfaced. The full scored
  candidate pool is passed to the split rather than a pre-truncated shortlist,
  giving each half the best possible selection to draw from.

- **Auto Headphones Cue** — Decks with their volume fader at or below 30% are
  automatically routed to the headphone/cue output for pre-listening. Between
  30% and 50% the send fades out linearly so there is a smooth handoff rather
  than a hard cut. The manual CUE button per deck still forces a full cue send
  and remains completely independent. The feature is configurable under
  Settings → Playback ("Auto Headphones Cue") and is on by default. It is
  automatically disabled when the master and cue outputs are the same device
  (to prevent double-monitoring).

### Fixed

- **Peak meters** — Fixed deck meters only showing for deck 4 and the master
  meter not appearing on the embedded device. Also fixes a frame-rate regression
  introduced with the meters in the previous release.

- **BPM tap tempo** (mesh-cue editor) — A new TAP button in the BPM row of
  the track editor continuously updates BPM from the average interval of the
  last eight taps. Tapping stops if there is more than a 3-second gap, so
  restarting at a different tempo just picks up immediately on the next tap.
  BPM is clamped to the valid range (20–250 BPM).

- **BPM range clamping** (mesh-cue editor) — Manual BPM edits via the text
  field and +/− buttons are now limited to a maximum of 250 BPM.

- **OTA update status display** — The in-app update status log now continues
  to populate even when the Settings panel is closed, so the progress feed is
  never silently dropped mid-install.

- **Audio device not applied at startup** — The selected master and cue output
  devices are now applied when mesh-player launches. Previously, both outputs
  used the system default until the user manually toggled the device selection
  in Settings.

- **NixOS cage restart racing to login TTY** — The cage-tty1 systemd unit
  now sets `restartIfChanged = false`, preventing nixos-rebuild's activation
  phase from issuing an untimely restart. After nixos-rebuild fully settles,
  mesh-update.service restarts cage cleanly via `ExecStartPost`. An explicit
  `Conflicts=autovt@tty1.service` is added as belt-and-suspenders against
  logind's on-demand VT activation.

---

## [0.9.11]

### Fixed

- **Track selection with duplicate titles** — Clicking a track no longer
  selected all other tracks sharing the same title (e.g. two different artists
  both with a track called "Everything"). Tracks are now identified by their
  database ID throughout the browser, so each entry in the list is always
  uniquely addressed regardless of title.

- **Deletion of tracks with duplicate titles** — Deleting a track now removes
  only the specific selected entry. Previously, deleting one of several
  same-titled tracks would silently fail entirely because the batch delete
  resolved tracks by title instead of by ID, finding no match once the ID-based
  NodeId format was in place.

- **Delete confirmation shows full track name** — The confirmation dialog now
  displays "Artist - Title" for each track instead of showing a raw internal ID.

---

## [0.9.10]

### Fixed

- **OTA update** — Fixed permission denied error when starting system updates on
  the embedded player. All privileged operations (file write, service start,
  cage restart, power off) now use sudo instead of direct filesystem writes
  and polkit authorization.

- **USB recording** — Fixed "failed to start recording" on the embedded player.
  USB sticks formatted as FAT/exFAT were mounted with root ownership, blocking
  the mesh user from creating the recordings directory. Automount now sets
  uid/gid for non-POSIX filesystems.

### Added

- **Set recording** — Record the master output to WAV files directly on connected
  USB sticks. Toggle recording in Settings → Recording. A pulsing red indicator
  in the header shows elapsed time while recording. When recording stops, a
  companion tracklist TXT file is automatically generated from the session history,
  listing each track played with timestamps relative to the recording start.
  Recordings are saved to `mesh-recordings/` on each connected USB stick.
  The recording thread uses a lock-free ring buffer and never blocks the audio
  thread — zero impact on playback performance.

- **MIDI learn tree** — The linear step-by-step MIDI learn wizard is now a
  collapsible tree you can navigate freely. Map your browse encoder first, then
  use it to scroll through sections, expand what you need, and skip what you
  don't. 73 mappable actions across 9 sections: Navigation, Modifiers,
  Transport, Performance Pads, Stems, Mixer, Effects, and Global Controls.

- **Live mapping during learn** — Mapped controls work immediately while you
  continue assigning others. Shift combinations are tracked live, so you can
  test your layout as you build it.

- **In-context descriptions** — Every mapping shows a short explanation when
  selected, so you know what each control does before assigning it. Setup
  questions (deck layout, compact mode, pad mode) also include descriptions.

- **Edit existing mappings** — Re-entering learn mode with an existing config
  pre-loads all assignments. Change individual mappings without re-doing
  everything. A verification screen shows what changed before saving.

- **Auto-start learn mode** — If no `midi.yaml` exists, learn mode opens
  automatically on startup.

### Improved

- **Stutter-free track loading** — Loading a track no longer causes the UI to
  freeze for 100–200 ms. Waveform peak data is now shared directly between the
  loader and the display without copying or converting, so waveforms grow
  smoothly while the track streams in. On ARM devices (RK3588S), decode workers
  are reduced from 4 to 2 to avoid saturating the shared memory bus, which
  previously caused GPU-side waveform stutter across all decks during loading.

- **Cyclic buffer pool** — Track audio buffers (~470 MB each) are now
  pre-allocated at startup and automatically recycled when a new track is loaded.
  This eliminates repeated large memory allocations during playback, reducing
  load times and preventing page-fault stutter on memory-locked embedded systems.
  Active on all platforms.

### Changed

- **Removed "4 Decks + Layer Toggle"** topology option (8 virtual decks is not
  a real-world configuration). Options are now: 2 Decks, 2 Decks + Layer
  Toggle, 4 Decks.

- **Removed per-deck Sync button** — Mesh handles sync globally; a dedicated
  sync button mapping is no longer offered.

---

## [0.9.9]

### Added

- **Pre-release OTA channel** — New toggle in Settings → System Update allows
  opting into release candidate and beta versions for over-the-air updates.
  When enabled, the update check queries all GitHub releases (not just stable),
  and the version comparator now correctly handles pre-release suffixes like
  `-rc.1` and `-beta.2`. Disabled by default.

- **Power off button (embedded)** — New "Power Off" button in Settings → System
  with a confirmation dialog to safely shut down the device. Only appears on
  embedded builds (`embedded-rt` feature flag). Includes polkit authorization
  for the mesh user to execute `systemctl poweroff` without a password.

- **DJ session history** — Mesh now records a full session history while you play,
  tracking every track load, play start, hot cue press, and loop usage. History is
  persisted to all active databases (local collection and every connected USB stick
  with a mesh collection), so your session data travels with your library.

- **Played track dimming in browser** — Tracks you've already played this session
  appear dimmed in the collection browser, giving you a visual cue to avoid repeats.

- **Played track exclusion from suggestions** — The smart suggestion engine
  automatically excludes tracks already played this session, ensuring all 30
  suggestion results are fresh picks.

- **Session metadata capture** — When loading a track from the suggestion panel,
  the suggestion score, reason tags, and energy direction are captured alongside the
  play record for future set analysis.

- **Bidirectional co-play tracking** — When two tracks are playing simultaneously
  (both audible with volume > 0), both tracks record each other in their
  `played_with` field, enabling future set reconstruction and transition analysis.

### Performance (embedded / OrangePi 5)

- **Fixed RT core pinning** — Rayon audio workers were incorrectly sharing 2 A55
  cores (4 workers on cores 2-3). Now 3 workers are pinned 1:1 to cores 1-3,
  with core 0 dedicated to the JACK RT thread. Eliminates core contention
  during parallel stem/mixer/multiband processing.

- **Background thread isolation** — All background threads (track loader, linked
  stem loader, preset loader, GC, HID I/O, LED feedback, DB writes) are now
  pinned to A76 big cores 4-7. This keeps A55 LITTLE cores exclusively for
  real-time audio and DSP, preventing background I/O from stealing cycles or
  polluting caches on latency-sensitive cores.

- **StemBuffer pool pre-allocation** — On embedded builds, 4 StemBuffers (~3.5 GB
  total) are pre-allocated and pre-touched at startup. Track loading checks out
  a buffer from the pool instead of allocating fresh memory, eliminating the
  ~452K page faults per load that caused TLB shootdown IPIs across all cores
  (including the RT audio core). After pool exhaustion, loading falls back to
  normal allocation.

- **PipeWire RT scheduling fix** — PipeWire's `nice.level` and `rt.prio`
  settings were placed in `context.properties` where they were silently ignored.
  Moved to `module.rt.args` so the RT module actually reads them. Also granted
  `LimitRTPRIO=95` and `LimitMEMLOCK=infinity` to the PipeWire and WirePlumber
  user services so they use direct RLIMIT scheduling instead of RTKit (which
  caps at SCHED_RR priority 20).

- **Kernel wakeup preemption tuning** — Added `sched_wakeup_granularity_ns=500µs`
  to match the existing CFS granularity, reducing scheduler latency for RT thread
  preemption.

- **cpu_dma_latency udev rule** — `/dev/cpu_dma_latency` is now group-accessible
  to `audio`, allowing mesh-player to disable CPU C-state transitions during
  playback without root.

- **cage-tty1 I/O and OOM hardening** — Set realtime I/O scheduling class
  (priority 0) so track file reads aren't starved by USB or journald activity.
  `OOMScoreAdjust=-1000` prevents the OOM killer from targeting the audio
  process.

### Fixed

- **Windows build: Linux-only USB sync** — USB export used `syncfs()` (a Linux
  syscall) without a platform gate, breaking the Windows cross-compile. Now gated
  with `#[cfg(target_os = "linux")]` and a `File::sync_all()` fallback on
  Windows/macOS for USB write safety.

- **Windows build: bindgen max_align_t conflict** — Phase 5 (mesh-cue) bindgen
  cross-compilation hit a `max_align_t` typedef redefinition between clang and
  mingw GCC headers. Fixed by defining `__GCC_MAX_ALIGN_T` to suppress the
  duplicate typedef.

- **Embedded theme not updating on OTA** — Theme file now force-deployed on every
  activation via NixOS activation script instead of tmpfiles copy-if-not-exists.

- **History DB schema on older USB databases** — Schema init now unconditionally
  creates all relations, treating "already exists" as success instead of pre-checking.

- **Present mode crash on Nvidia/X11** — Desktop wrapper scripts and devshell
  now use `auto_vsync` (tries Mailbox, falls back to Fifo) instead of
  hardcoded `mailbox` which panics on GPUs that don't support it. Embedded
  kiosk keeps `mailbox` where the hardware is known.

- **History writes moved off UI thread** — Database writes for session history
  now run on background threads (fire-and-forget). Only session-end writes are
  synchronous to ensure data is flushed before exit.

- **Fast track dimming** — Browser dimming now uses cached track paths instead
  of querying the database for every visible track. Previously, refreshing
  dimming state for ~200 tracks triggered ~200 full folder queries on the
  UI thread, causing multi-second freezes.

- **Clean process exit** — The application now calls `process::exit()` after
  shutdown to terminate lingering background threads (rayon pool, history
  writers) that previously kept the process alive indefinitely after Ctrl+C.

- **Settings MIDI navigation unreachable entries** — Auto-Gain, Target LUFS,
  and Slicer Buffer settings were unreachable via MIDI encoder press due to a
  hardcoded entry count (`base_entries = 13`) that fell behind the actual count
  of 16 navigable items. Pressing the encoder on these entries would trigger
  the wrong action (e.g. opening the Network sub-panel instead of toggling
  Auto-Gain).

- **OTA version check always reported "up to date"** — The GitHub API response
  parser used the wrong string split index (`nth(2)` instead of `nth(1)`),
  causing it to extract a comma instead of the version tag. Every version
  check silently returned "up to date" regardless of actual available updates.

- **Smart suggestion loading spinner stuck** — The suggestion loading indicator
  could get stuck permanently when the energy direction slider was adjusted
  with no audible decks, or when seed tracks hadn't changed between refreshes.
  The spinner now clears correctly in all edge cases.

### Improved

- **Data-driven settings registry** — The settings UI is now driven by a single
  `Vec<SettingsItem>` registry that serves as the source of truth for both view
  rendering and MIDI navigation. Adding or reordering settings only requires
  editing `build_settings_items()` — no manual index counting, no hardcoded
  entry offsets, no separate view functions to keep in sync. Replaces 4
  previously fragile sync points (entry builder, entry count, 19 hardcoded
  wrap indices, MIDI handler index arithmetic) with behavior-based dispatch.

---

## [0.9.8]

### Added

- **Configurable fonts with settings selector** — Six bundled fonts selectable in
  Settings → Display: Hack, JetBrains Mono, Press Start 2P, Exo, Space Mono, and
  Sax Mono. Default is Space Mono. Font data is compiled in via `include_bytes!()`
  and registered with iced at startup. Changing font requires restart. Per-font
  size scaling normalizes visual appearance (pixel fonts like Press Start 2P are
  scaled down automatically). Both mesh-player and mesh-cue share the `AppFont`
  enum from mesh-widgets.

- **Global font size scaling** — Three size presets (Small, Medium, Big) in
  Settings → Display scale all UI text via a global `sz()` multiplier applied to
  every explicit text `.size()` call across mesh-player, mesh-cue, and mesh-widgets.
  Small = 1.0× (original sizing), Medium = 1.2× (default), Big = 1.4×. The scale
  factor is stored in an `AtomicU32` set once at startup and combined with per-font
  normalization so all fonts render at visually equivalent sizes regardless of preset.

- **Window icon and header branding** — Both apps show `grid.png` as the window
  icon (taskbar/dock) and as a logo image in the header bar. Header text changed
  to "mesh" in both apps. Logo uses a `LazyLock<image::Handle>` static to avoid
  per-frame texture re-upload flickering.

- **YAML-based theme system** — Themes are now defined in `theme.yaml` in the collection
  folder. Each theme specifies a full iced UI palette (background, text, accent, success,
  warning, danger) plus 4 stem waveform colors. Ships with 5 built-in themes: Mesh
  (indigo-black with cyan-blue accent), Catppuccin Mocha (pastel on dark blue), Rosé Pine
  Moon (editorial purple-blue), Synthwave (neon retro), and Gruvbox (warm earthy). The
  file is auto-created on first launch and old-format files are auto-migrated. Users can
  add custom themes by editing the YAML. Both mesh-player and mesh-cue load themes
  dynamically and offer a theme picker in settings. Replaces the hardcoded
  `StemColorPalette` enum and legacy `theme.rs` OnceLock system.

- **Window minimum size** — mesh-player enforces 1280×720 and mesh-cue enforces 960×600
  minimum window dimensions to prevent layout breakage on resize.

- **Hot cue count column in track browser** — New narrow "Q" column between # and Name
  shows an orange pill with the number of hot cues set per track, letting DJs see at a
  glance which tracks are prepared. Empty cell when no cues are set. Sortable. Works in
  both mesh-cue and mesh-player, including USB browsing.

- **D key sets drop marker** — Pressing `D` sets or moves the drop marker to the
  current playhead position. `Shift+D` clears it. Configurable via `keybindings.yaml`.

- **Progressive track loading** — Both mesh-cue and mesh-player now use a
  work-stealing thread pool (4 workers) that prioritises hot cue regions first,
  then fills the remaining audio in ~10-second chunks. Waveforms visibly grow
  as each chunk completes. Hot cue areas are playable within the first few
  hundred milliseconds; the rest of the track loads progressively in the
  background without blocking the UI.

- **Configurable column editability** — Track table columns are now editable per-app
  via `editable_columns` on `TrackTableState`. mesh-cue enables double-click editing
  for Name, Artist, BPM, and Key columns; mesh-player leaves all read-only.

### Fixed

- **Sub-beat loops (1/8, 1/4, 1/2) silently fail** — `snap_to_beat()` snapped the loop
  end back to the same beat as the start, producing a zero-length loop that was ignored.
  Added `snap_to_subdivision()` which divides each beat grid interval into fractional
  parts matching the loop length, keeping sub-beat loops phase-locked to the grid.

- **Encoder scroll wraps to top on duplicate track IDs** — When the same track appeared
  at multiple positions in a playlist, the encoder scroll handler used `.position()` to
  reverse-lookup the selected ID, always finding the first occurrence. Scrolling past the
  second copy jumped back to the first. Now tracks position directly via a stored index.

- **Cue count pill stretches to fill column width** — The orange hot cue pill expanded to
  the full 28px column instead of staying compact. Fixed by using `Shrink` centering.

- **Scratch playhead drifts forward instead of following mouse** — During waveform
  scratching the playhead visually drifted ahead of the mouse cursor because the
  UI's inter-frame interpolation kept extrapolating forward at normal playback rate.
  Now sets playback rate to zero while scratch is active so the playhead tracks the
  mouse position exactly.

- **Track name parsing: number prefix leaking into artist** — Filenames with UVR5
  playlist+track-number prefixes (e.g., `1_01 Black Sun Empire - Feed the Machine`)
  left the track number in the artist field. Added a compound strip that only removes
  bare space-separated track numbers when a UVR5 prefix was present, avoiding false
  positives on legitimate names like "808 State".

- **Browser jumps to top after deleting a track** — `handle_confirm_delete()` called
  `clear_selection()` after refreshing the track list, leaving no selection and resetting
  scroll to the top. Now captures the selected index before deletion and re-selects the
  neighbor at that position (clamped to list bounds) after refresh.

- **USB stick removal leaves stale browser state** — `remove_usb_device()` checked
  `active_usb_idx` after `retain()` had already removed the device, so the check always
  failed and the browser never cleared. Fixed ordering to check before removal. Also
  wires up the existing `clear_usb_database()` function which was implemented but never
  called on disconnect.

- **MIDI/HID devices not detected when connected after launch** — Device enumeration
  only ran at startup. Added `check_new_devices()` to the existing 2-second poll loop,
  scanning for expected-but-unconnected devices from `midi.yaml`. Reuses the existing
  `try_connect_all_midi`/`try_connect_all_hid` which skip already-connected devices.

- **Slicer preset trigger uses wrong preset index** — `SlicerTrigger` handler checked
  which stems had patterns using the globally-selected editor preset instead of the
  preset corresponding to the pressed pad. Now uses `button_idx` directly, matching
  the index sent to the audio engine.

- **Slicer waveform not fixed in shader** — `build_uniforms()` always centered the
  zoomed window on the playhead, ignoring `FixedBuffer` view mode. In slicer mode
  the window now locks to the slicer buffer bounds so the waveform stays fixed and
  the playhead moves left-to-right across it.

- **Slicer LED feedback param mismatch** — `deck.slicer_slice_active` feedback looked
  for a `"slice"` parameter but the MIDI learn system generates `"pad"`. Fixed to
  match.

- **USB export stuck on preset sync** — `copy_dir_all()` was missing `sync_all()`
  after each file copy, leaving preset YAML data in kernel page cache instead of
  flushing to USB flash media. Subsequent phases would block 2-3 minutes waiting
  for implicit writeback. Now explicitly fsyncs each file, matching the existing
  `copy_large_file()` pattern used for WAV track copies.

- **Duplicate track import** — Batch import now detects tracks that already exist
  in the collection (by checking the output FLAC path) and skips them, avoiding
  redundant stem loading, BPM/key analysis, ML inference, and FLAC re-export.
  Applies to both pre-separated stem import and mixed-audio separation paths.

- **Volume dim applied to overview waveform** — The WGSL shader volume-dim overlay
  darkened both the zoomed and overview waveforms when deck volume was lowered.
  Added `!is_overview` guard so only the zoomed waveform dims; the overview stays
  at full brightness for navigation clarity.

- **Cue panel layout misaligned with transport/stems** — The DROP button sat at the
  right end of the hot cue row with 10px padding, misaligning with the transport
  column above. Moved DROP to a 120px container on the left (matching transport
  width), 8 hot cue buttons fill the center, and a 56px right spacer aligns with
  the stem column. Stem link buttons now have 2px vertical spacing.

- **Delete with non-default sort jumps to wrong position** — After deleting a track
  while sorted by BPM/key/etc., the selection jumped to the wrong neighbor because
  `get_tracks_for_display()` returned unsorted data but `select_at_index()` used the
  old sorted index. Added `refresh_tracks()` helper that re-applies the current sort
  after every track list refresh.

- **Multi-select drag-and-drop blocks UI** — `add_tracks_to_playlist()` ran O(4N)
  database queries (resolving the target playlist and querying max sort order per
  track). Hoisted both invariant queries before the loop, reducing to O(N+2).

- **Track title not editable** — The Name column was excluded from
  `TrackColumn::is_editable()`. Added `Name → "title"` DB field mapping and
  included Name in mesh-cue's editable column set.

- **Multi-selection not cleared on click without drag** — Clicking an already-selected
  track preserved the multi-selection for potential drag, but releasing without dragging
  never collapsed it back. Now detects click-release without drag threshold and selects
  only the clicked track. Clicking empty table space clears all selection.

- **Stale playhead shown during track load** — Loading a new track showed the previous
  track's playhead position on the new waveform because `DeckAtomics.position` wasn't
  reset after `unload_track()`. Now zeroes position and timestamp atomics immediately.

- **USB export re-copies all FLAC files instead of skipping existing** — The skip
  check compared extension-stripped stems against full `.flac` filenames on USB,
  never matching. Fixed path comparison to use consistent extensions.

- **Audio crackling during USB export** — The CPAL audio backend continued running
  while the export background thread performed heavy sequential I/O to USB media,
  starving the audio callback of CPU time and causing constant crackling. Now pauses
  the audio streams when export starts and resumes them on completion, error, or
  cancellation.

- **Editable cells swallowing clicks and shift-click collapsing selection** —
  Double-click on editable columns (Name, Artist, BPM, Key) consumed the click
  event at the cell level, preventing row-level handlers from firing. Shift-click
  on an already-selected track collapsed the multi-selection. Fixed cell-level
  mouse areas to properly propagate events and preserve selection state.

- **Double-click edits cell instead of loading track** — Double-clicking an
  editable column entered edit mode because the first click of the double-click
  selected the row, making the "already selected?" check always true. Now
  double-click always activates (loads) the track. Cell editing is entered by
  single-clicking an editable cell on an already-selected row (via `CellClicked`
  message that checks selection state before the click modifies it).

- **Shift-click selection and drag start very slow on large collections** —
  Shift-click selection cloned all track IDs into a temporary Vec on every click,
  then did O(n) linear searches. Drag initiation called `get_node()` per selected
  track, each doing a database query + linear search (O(n × folder_size)). Fixed
  by passing track row references directly to range selection (zero-copy) and
  resolving drag names from already-loaded in-memory track data instead of the DB.

- **Dropping many tracks to playlist is very slow** — Adding N selected tracks
  to a playlist ran 5N+2 database queries (full metadata load per track plus
  individual inserts). Replaced with batch path→ID resolution and batch insert,
  reducing to 4 queries regardless of selection size. Also skips refreshing
  browser panes not showing the target playlist.

- **Deleting or removing many tracks very slow** — Deleting N tracks from the
  collection or removing N from a playlist ran O(N × folder_size) database
  queries (each track loaded its entire folder/playlist to find one title).
  Batch methods now group by folder/playlist, load once, resolve all titles
  to DB IDs, then batch-delete in a single query. Confirmation dialog also
  resolves names from NodeId directly instead of per-track DB lookups.

### Changed

- **Reworked Mesh theme** — New earthy warm palette with violet (`#B090E0`) primary accent
  and olive green (`#707030`) secondary. Dark warm background (`#202010`), muted stem colors:
  olive vocals, teal-blue drums, dark red bass, violet other.

- **Header "mesh" text bolder and larger** — The "mesh" title in both apps is
  now bold weight and 25% larger (size 24 → 30) for better visual presence.

- **Deck header badge left padding** — The space to the left of the deck number
  badge is now 10px, matching the stem indicator strip width for visual alignment.

- **Header layout: FX selector centered, BPM repositioned** — In mesh-player,
  the global FX preset selector and BPM are truly centered in the header using
  a stack overlay (immune to asymmetric left/right content widths). The BPM
  slider is hidden in performance mode but remains visible in mapping mode
  (`--midi-learn`) for interactive adjustment.

- **"General Collection" renamed to "Collection"** — The tree browser root node
  for the track collection now displays as "Collection" instead of "General
  Collection".

- **Deck header text sizing** — Increased all header text sizes to better fill
  the 48px header height: track name 20→24, BPM/loop/LUFS 16→20, key 18→22,
  badge number 22→26. Badge now fills full header height (was 38px with 10px
  gap). Added more horizontal spacing (12→18px) between right-side info items.

- **Track display name format** — Deck headers and waveform overlays now show
  `{Artist} - {Name}` from parsed metadata instead of raw filenames. Falls back
  to filename without extension when metadata is unavailable. Added `name` field
  to `TrackMetadata` and `display_name()` method to `LoadedTrack`.

- **Waveform zoom-out subsampling** — Changed resolution scaling curve from
  linear to quadratic and lowered minimum resolution from 256 to 128 pixels.
  Reduces visual jitter at maximum zoom-out (64 bars) while preserving detail
  at moderate zoom levels.

- **USB export uses batch syncfs** — Per-file `sync_all()` calls replaced with a
  single `syncfs()` after all track files are copied. Reduces USB I/O overhead
  from O(N) sync operations to O(1).

---

## [0.9.7]

### Added

- **Embedded RT audio optimizations (Phase 1+2)** — Comprehensive real-time
  audio tuning for the OrangePi 5 (RK3588S) embedded image. Phase 1: kernel
  boot params (`rcu_nocbs`, `nohz_full`, `irqaffinity`, `transparent_hugepage=never`,
  `nosoftlockup`, `nowatchdog`), locked PipeWire quantum to 256, PipeWire RT
  module (priority 88), IRQ affinity service (audio IRQs → A55 core 0, all
  others → A76), disabled irqbalance, deep idle state disable on A55 cores,
  system service CPU pinning (NetworkManager/journald → A76), BFQ I/O scheduler
  for USB storage. Phase 2: `embedded-rt` feature flag with `mlockall()` to
  prevent page faults, `/dev/cpu_dma_latency=0` to disable C-states, SCHED_FIFO
  priority 70 for rayon audio workers, CPU affinity pinning (rayon → A55 cores
  2-3), 512KB stack pre-faulting, RT capability verification at startup. All
  application code is feature-gated (`#[cfg(feature = "embedded-rt")]`), auto-
  enabled on aarch64 builds only.

- **Resource monitoring in header** — CPU%, GPU%, RAM usage, and FPS counter
  displayed in the player header bar. GPU utilization reads Mali devfreq
  (aarch64) or AMD DRM sysfs (x86). FPS counted from iced frame events.
  Polls at 500ms intervals via `ResourceMonitor` in mesh-core (reusable by mesh-cue).

- **Mali linked stem split view** — Overview waveforms on the Mali shader path
  now show linked stems as a split view (active stem top half, inactive bottom
  half). Precomputed on CPU into the existing 4-stem buffer with signed
  min/max encoding — no shader changes or GPU upload increase needed.

- **Canvas stem mute & link indicators** — Zero-GPU stem status indicators
  rendered as iced container widgets beside the zoomed waveform. Mute column
  (always visible) + link column (when any stem linked). Replaces removed
  Mali shader indicators with no GPU cost.

- **Waveform abstraction setting** — New "Waveform Abstraction" option (Low,
  Medium, High) controlling the grid-aligned subsampling strength per stem. Low
  gives near-raw peak rendering, Medium (default) provides tuned per-stem
  abstraction (vocals smooth, drums detailed), High pushes further toward a
  stylized look. Takes effect immediately.

- **Render debug logging** — Added `[RENDER]` debug log entries throughout the
  waveform pipeline: peak computation at load time (computed peaks-per-pixel at
  reference zoom), and per-frame shader uniforms (zoom level, peak density,
  abstraction, blur). Visible with `RUST_LOG=debug`.

### Changed

- **Cage CPU affinity widened** — `CPUAffinity` changed from `4-7` (A76 only)
  to `0-7` (all cores) so mesh-player can set per-thread affinity internally:
  audio RT + UI → A55 (deterministic in-order), background loading → A76 (high
  throughput).

- **Collection track format: WAV to FLAC** — Stem files now use 8-channel FLAC
  lossless compression instead of raw WAV. ~58% file size reduction (e.g., 240 MB
  WAV → 104 MB FLAC) with zero audio quality loss. Encoding via `flacenc` crate,
  decoding via symphonia. Existing collections must reimport tracks (delete
  `tracks/` folder and reimport).

- **In-memory audio file reader** — `AudioFileReader` now reads the entire file
  into memory (`Arc<[u8]>`) on open, then creates independent symphonia decoders
  per region from the shared buffer. All I/O happens once at open; subsequent
  region reads are pure CPU decode with no file system access.

- **Simplified peak interpolation** — Removed the hybrid max-hold/bilinear
  interpolation in `sample_peak()`. The old approach blended between interpolated
  and preserved (max-hold) values based on peak magnitude, adding complexity
  without clear visual benefit now that peak resolution is much higher. The new
  code uses straightforward bilinear interpolation between grid points.

### Performance

- **Instant grid render on track load** — Beat grid, cue markers, loop regions,
  playhead, and stem indicators now render immediately when a track is loaded,
  instead of waiting for peak data to arrive from the background loader. The
  shader's early-exit guard was split so only stem envelope rendering (which
  genuinely needs peak data) is gated behind peak availability. A pulsing
  brightness overlay signals that audio is still loading and interaction is ready.

- **Parallel priority region decode** — Priority regions (around cue points) are
  now decoded in parallel via `std::thread::scope`. Each thread creates its own
  decoder from the shared `Arc<[u8]>` buffer. Results are merged sequentially
  after all threads complete. First playable audio arrives ~3x faster.

- **Parallel gap decode** — Non-priority gap regions are also decoded in parallel,
  removing the old sequential sub-batching loop. Combined with priority parallelism,
  total decode time drops from ~4s to ~1.5s on 4+ cores.

- **Full LTO for release builds** — Changed from thin LTO to full LTO
  (`lto = true`) for maximum cross-crate optimization in release binaries.

- **Native CPU targeting** — Added `target-cpu=native` to RUSTFLAGS for all build
  targets (NixOS, deb container, Windows cross-compile). Enables host-specific
  SIMD extensions (AVX2, SSE4.2, NEON) for decode and analysis hot paths. Aarch64
  cross-compilation uses `cortex-a76` (RK3588) instead of native.

- **Parallel track loading** — Track loader now dispatches each load request to
  rayon's thread pool instead of processing sequentially on a single thread.
  All 4 decks load simultaneously when loading tracks in parallel.

- **Linked stem stretch threads** — `MAX_STRETCH_THREADS` increased from 2 to 8.
  Pre-stretching runs at nice(10) priority so JACK audio thread preempts safely.

- **Dynamic waveform peak resolution** — Highres peak count is now proportional to
  actual audio length and BPM instead of a fixed 65K constant. A BPM-aware formula
  targets 1 peak per pixel at 4-bar zoom (the closest practical zoom level).
  Short tracks allocate proportionally less memory.

- **GPU buffer vec2 packing** — Waveform peak storage changed from `array<f32>` to
  `array<vec2<f32>>` in the WGSL shader, halving the number of buffer reads per
  peak lookup. The CPU-side interleaved `[min, max, ...]` layout is bit-identical
  to `vec2<f32>`, so no data conversion is needed.

### Fixed

- **FLAC block-size padding** — Work around flacenc-rs#242 where
  `encode_with_fixed_block_size()` produces malformed final frames when sample
  count is not a multiple of the block size (default 4096). Samples are now padded
  to the next block-size boundary with silence before encoding.

- **Nix build missing .cargo/config.toml** — The Nix source filter excluded
  `.cargo/config.toml`, meaning NixOS builds never received `--export-dynamic`
  (needed for PD externals) or `target-cpu=native`. Added `config.toml` to the
  filter for both `mesh-build.nix` and `mesh-player.nix`.

- **Prelinked stems missing from waveform** — Linked stems loaded asynchronously
  (prelinked in track metadata) were not shown in overview or zoomed waveforms.
  `TrackLoadResult::Complete` was replacing the entire `OverviewState`, discarding
  linked stem peaks that arrived earlier from the async loader. Fix preserves
  linked stem data across the state replacement and rebuilds GPU buffers.

- **Settings MIDI navigation indices** — Fixed `next_idx` for dynamic settings
  sections (Network, System Update, MIDI Learn) to match the actual entry count,
  preventing index collisions during MIDI encoder navigation.

- **Track drift from FLAC seek overshoot** — Symphonia's FLAC decoder seeks to
  the nearest block boundary (every 4096 samples), not the exact requested frame.
  Parallel region decoding did not account for this, leaving up to 4095 extra
  leading samples per region. Over multiple seeks this caused audible sync drift.
  The decoder loop now skips leading frames based on the `SeekedTo` return value.

- **Beat grid integer truncation** — `regenerate_with_rate()` cast
  `samples_per_beat` to `u64`, truncating the fractional part. At 174 BPM this
  accumulated ~7.5 ms drift over 500 beats. Replaced with f64 accumulation and
  per-beat rounding (max error ±0.5 samples, never accumulates).

- **FLAC padding inflating duration** — The FLAC encoder pads to block-size
  boundaries, inflating `total_samples` in the stream header by up to 4095
  samples. `frame_count` and `duration_samples` are now capped at the
  metadata-derived duration from the database.

- **USB linked stem metadata lookup** — Linking a stem from a USB track (e.g.
  via smart suggestions from another USB stick) silently fell back to 120 BPM
  defaults because `LoadedTrack::load_to()` passed absolute paths to USB
  databases that store relative paths. Introduced `resolve_track_metadata()` as
  the single source of truth for path-aware DB resolution: tries local DB first,
  then detects USB collection roots, strips the prefix, and queries the correct
  USB database. The linked stem loader and domain layer now delegate to this
  function instead of duplicating path resolution logic.

- **Linked stem BPM source** — `confirm_stem_link_selection()` used the
  global master BPM instead of the host deck's native track BPM for
  time-stretching linked stems. This caused incorrect stretch ratios when the
  master tempo differed from the host track's original BPM.

- **Redundant Complete re-decode** — The streaming loader's `Complete` path
  redundantly re-computed all waveform peaks (~200 ms) and replaced the
  incrementally-built overview state, requiring a fragile linked-stem
  preservation hack. `Complete` now carries an `incremental` flag; when true
  (streaming path), the handler skips state replacement and redundant stem
  upgrades.

### Removed

- **Unified waveform rendering pipeline** — Removed the dual-path GPU shader
  architecture. The CPU-precomputed "Mali" path (1:1 peak per pixel, zero GPU
  reduction loops) is now the only rendering pipeline for all platforms. This
  eliminates:
  - **Desktop shader** (`waveform.wgsl`, 834 lines) with its GPU-side
    `minmax_reduce` loops (up to 64 iterations per pixel per stem)
  - **`mali-shader` feature flag** from all three `Cargo.toml` files and the
    Nix build (`mesh-player.nix`)
  - **~18 `#[cfg]` gates** in the shader Rust code (`mod.rs`, `pipeline.rs`)
  - **6 settings** that only affected the desktop shader: Waveform Quality,
    Motion Blur, Depth Fade, Depth Fade Inverted, Peak Width, Edge AA — along
    with their config enums, draft state fields, handler arms, and UI sections
  - **5 `PlayerCanvasState` fields**: `motion_blur_level`, `depth_fade_level`,
    `depth_fade_inverted`, `peak_width_mult`, `edge_aa_level`
  - **Engine command** `SetWaveformQuality` and domain method
    `set_waveform_quality()`
  - **Unused peak functions**: `smooth_peaks()`, `smooth_peaks_gaussian_wide()`,
    `GAUSSIAN_WEIGHTS_17`
  - Quality level hardcoded to 0 (Low) throughout the loader pipeline
  - Settings entry count reduced from 20 to 14, MIDI nav indices renumbered
  - Total: **~1,550 lines deleted** across 23 files, zero new code

- **WAV chunk parsers** — Removed `parse_mlop_chunk()`, `parse_mslk_chunk()`,
  `serialize_mslk_chunk()`, and `align_peaks_to_host()` — legacy WAV custom chunk
  handling no longer needed with FLAC format.

- **Waveform preview from file** — Removed `WaveformPreview`, `from_preview()`,
  and `read_waveform_preview_from_file()`. Waveform peaks are now computed from
  decoded audio during the streaming load, not read from embedded file chunks.

---

## [0.9.6]

### Performance

- **Mali GPU hyper-optimized waveform shader** — New `waveform_mali.wgsl` shader
  variant for Mali Valhall GPUs (Orange Pi 5 / RK3588). Reduces per-pixel ALU cost
  from ~1,320 to ~200 ops by removing depth fade, peak width expansion, stem
  indicators, playhead glow, motion blur branching, and `fwidth()` derivative calls.
  Replaces `smoothstep` with linear clamp AA and `dpdx`/`dpdy` derivatives with
  analytical slope estimation from adjacent peaks.

- **CPU-precomputed waveform peaks** — On Mali builds, peak subsampling (grid-aligned
  min/max reduction) is computed on the CPU instead of the GPU. Each pixel column gets
  exactly one precomputed (min, max) pair per stem, guaranteeing the 1:1 peak-per-pixel
  invariant at ALL zoom levels (not just 4-bar). This eliminates the `minmax_reduce`
  loop from the shader entirely — the GPU does a single buffer read per stem per pixel.
  CPU cost is ~0.6ms/frame on an A76 core; upload cost is ~40KB per view.

- **Draw call skip for empty decks** — Unloaded decks now skip the GPU draw call
  entirely (checked via `has_track` uniform), avoiding TBDR tile binning overhead on
  Mali's tiled renderer.

### Added

- **`mali-shader` Cargo feature flag** — Enables the Mali-optimized shader and CPU
  peak precomputation. Automatically activated on aarch64 nix builds; can be enabled
  on x86 for testing with `--features mali-shader`. Propagated through mesh-player
  and mesh-cue Cargo.toml. *(Removed in 0.9.7 — Mali path became the universal
  default.)*

---

## [0.9.5]

### Improved

- **Stem link LED feedback** — Stem mute LEDs now toggle between two color shades
  when a linked stem is present: primary shade for the original, alternate shade for the
  linked version. Stems with a linked counterpart pulse subtly to signal interactivity.

- **Darker stem LED colors** — Redesigned stem LED colors for better contrast and F1
  HID compatibility (7-bit, 0-125 range): dark green (vocals), deep navy (drums),
  rusty orange (bass), violet (other).

- **Auto-open browser on stem link** — Pressing shift+stem on an unlinked stem now
  automatically opens the browser overlay and activates browse mode so the encoder
  navigates the track list. The selected track is highlighted in the stem's color.

### Fixed

- **Linked stem visual LUFS scaling** — Linked stem waveforms were double-corrected
  for LUFS (once baked into peak buffer, once in shader). Now normalizes linked→host
  level only; the shader handles host→-9 LUFS uniformly for both original and linked.

- **JACK xruns during linked stem loading** — Time stretching for linked stems used
  up to 4 threads, saturating all CPU cores and starving the JACK audio callback.
  Reduced to 2 threads with lowered scheduling priority (`nice 10`) to leave headroom
  for real-time audio processing.

---

## [0.9.4]

### Performance

- **GPU-accelerated waveforms** — Zoomed waveform rendering moved from CPU to GPU.
  Waveform data is uploaded once when a track loads; only the playhead position and
  display state are sent each frame. Dramatically reduces CPU usage during playback,
  especially at high refresh rates with multiple decks.

- **Smarter redraw scheduling** — Waveform display only redraws when something
  actually changes, instead of rebuilding every frame unconditionally.

- **Removed background peaks thread** — Eliminated a legacy background thread that
  recomputed zoomed waveform peaks every tick. The GPU shader reads peak data uploaded
  once at track load, making this thread pure overhead.

### Improved

- **Smoother waveform appearance** — Waveforms now have a cleaner, more abstract look
  with per-stem detail tuning. Bass is the smoothest, drums retain more detail, and
  vocals/other sit in between. Thin peaks render with proper anti-aliasing instead of
  flickering between pixel rows.

- **Playhead brightness gradient** — Waveform peaks near the playhead are subtly
  brighter, with an inverse-exponential falloff so the effect is concentrated around
  the current position. Peak edges glow more than centers for a natural depth effect.

- **Overview window indicator** — The overview waveform now highlights the region
  currently visible in the zoomed view with a subtle overlay.

- **Red downbeat markers** — Bar lines in the beat grid are now red to distinguish
  them from regular beat lines, matching the overview waveform style.

- **LUFS-normalized waveform amplitude** — All tracks are visually scaled to match
  -9 LUFS, so quiet and loud tracks appear at the same visual amplitude in the
  waveform display.

- **Slicer shows 16 divisions** — Slicer overlay now correctly displays 16 slice
  divisions instead of 8. The currently playing slice is highlighted with an orange
  tint, and the next slice boundary has a yellow accent.

- **Beat grid respects density setting** — The overview waveform beat grid now follows
  the grid density setting (8, 16, 32, or 64 beats between red markers). Each period is
  subdivided into 4 equal parts (1 red + 3 gray) for consistent visual rhythm. Overview
  grid lines are subtler to avoid clutter. Zoomed view shows individual beat lines.

- **BPM-aligned overview waveforms** — Overview waveforms are now scaled so that beat
  markers align across all loaded decks. The longest track (in beats) fills the full
  width, and shorter tracks are padded proportionally.

- **GPU waveforms in mesh-cue** — The track editor now uses the same GPU shader
  waveform renderer as the player, replacing the old CPU canvas rendering.

- **Stem mute indicators restored** — Zoomed waveform shows colored rectangles on the
  outer edge indicating each stem's mute state (bright = active, dark = muted). Indicators
  appear on the left edge for decks 1 and 3, right edge for decks 2 and 4.

- **Linked stem indicators in waveform** — When a stem has a linked stem loaded, a second
  indicator column appears next to the mute indicators. Shows full color when the linked
  stem is active, dimmed when inactive. Replaces the diamond symbols in the header.

- **Linked stem waveform toggling** — Zoomed waveform now visually switches to the linked
  stem's peaks when a linked stem is activated, matching the audio output. Overview waveform
  shows a mirrored split: active stem peaks go upward from the center line, inactive
  alternative peaks go downward (dimmed), so you can see both versions at a glance. Peak
  buffers are cached and rebuilt only when linked stem data arrives, with toggle display
  handled entirely by GPU uniforms for instant visual response.

- **Overview split rendering** — Non-linked stems in split mode now render only on the
  top half of the overview waveform. The bottom half is reserved exclusively for linked
  stem alternatives, giving a cleaner visual separation.

- **MIDI shift+stem mute toggles linked stems** — Pressing shift + a stem mute button on
  a MIDI controller now toggles the linked stem, matching the UI behavior. Uses a dedicated
  `deck.stem_link` action resolved by the mapping engine, eliminating reliance on UI-side
  shift state synchronization.

### Fixed

- **Buttery-smooth playhead scrolling** — Playhead interpolation now uses timestamps
  from the audio thread with playback rate compensation, eliminating the rhythmic
  micro-stuttering caused by audio buffer quantization.

- **Correct stem overlap rendering** — Fixed alpha blending from premultiplied to
  straight alpha, eliminating the white/washed-out outlines where stems overlap.

- **Waveform stays in sync with audio** — Fixed two sources of visual drift that caused
  the zoomed waveform to gradually fall out of sync with the audio over longer tracks.

- **Beat grid always visible** — Beat grid lines now appear for all tracks, including
  those without detailed beat analysis (falls back to BPM-based grid).

- **Stable waveform at all zoom levels** — Waveform no longer jumps or wobbles when
  changing zoom level or at deep zoom.

- **Waveform loads progressively** — Overview waveform fills in as the track loads
  instead of appearing all at once.

- **Playhead stays centered at track edges** — Zoomed waveform no longer snaps the
  playhead off-center when near the beginning or end of a track.

- **Overview waveform stays visible after loading** — Fixed a bug where the overview
  waveform would appear during progressive loading but disappear once loading completed.

- **Beat markers no longer too thick** — Reduced beat grid line thickness and opacity
  for a cleaner look that doesn't obscure the waveform.

- **Smooth waveform scrolling in mesh-cue** — Replaced fixed 16ms timer with
  display-synced frame scheduling, and fixed playhead interpolation to only reset when
  the audio position actually changes. Eliminates bursty waveform movement caused by
  audio buffer quantization.

- **Beat grid visible on all track lengths** — Fixed beat grid disappearing on longer
  tracks due to an overly aggressive rendering threshold in the GPU shader.

- **Smooth cue preview waveform** — Zoomed waveform now scrolls smoothly during cue and
  hot cue preview, matching the smoothness of normal playback.

- **Settings auto-save on close** — Closing the settings panel (via UI or MIDI controller)
  now automatically saves any changed settings to disk. Previously, changes made via MIDI
  encoder were applied in-memory but lost on restart because the async save task was
  discarded.

---

## [0.9.3]

### Performance

- **Rendering: display-synced frame scheduling** — Replaced hardcoded 60Hz timer
  (`time::every(16ms)`) with `window::frames()`, which fires at the compositor's
  native vblank rate. Automatically adapts to 60Hz, 120Hz, or any display refresh
  rate without code changes. Previously, 120Hz displays were capped at 60fps.

- **Rendering: canvas geometry caching** — Added `canvas::Cache` to
  `PlayerCanvasState`, eliminating per-frame reconstruction of all waveform
  geometry (~100+ draw ops, 32 Vec allocations, 16 Path closures per frame across
  4 decks). Cache invalidates on visual state changes (playhead, volume, stem
  mute, loop, etc.) and skips reconstruction entirely when paused. At 120Hz this
  prevents ~12,000 unnecessary draw operations per second during idle.

- **Rendering: Mailbox present mode** — Set `ICED_PRESENT_MODE=mailbox` as
  default across all environments (devshell, embedded kiosk, Debian/RPM packages).
  Mailbox uses a single-frame queue (~8ms latency at 120Hz) vs Fifo's 3-frame
  queue (~25ms). Wayland compositors guarantee tearless presentation regardless.

- **Rendering: Vulkan backend** — Set `WGPU_BACKEND=vulkan` as default everywhere,
  replacing GLES on embedded (which couldn't use Mailbox). Vulkan is required for
  Mailbox present mode and enables `PowerPreference::HighPerformance` GPU selection.
  On embedded, uses PanVK (Mali-G610, Vulkan 1.2+ conformant).

- **Rendering: MSAAx4 antialiasing** — Enabled `.antialiasing(true)` for smooth
  waveform line rendering. Also ensures `PowerPreference::HighPerformance` for GPU
  adapter selection via wgpu.

- **Rendering: OTA journal polling gated** — Journal polling for OTA updates now
  only runs when the settings modal is open AND an update is installing. Previously
  polled every frame unconditionally, adding unnecessary work to the render loop.

### Changed

- **Window: default size 1920x1080** — Default window size increased from 1200x800
  to 1920x1080 (Full HD). Auto-detection via `monitor_size()` is attempted at
  startup but returns `None` on Wayland tiling WMs (known winit limitation). On the
  target cage kiosk, the window auto-fills the display regardless.

- **Packaging: Vulkan wrapper scripts** — Debian and RPM packages now install
  binaries to `/usr/lib/mesh/` with a thin wrapper at `/usr/bin/` that sets
  `WGPU_BACKEND=vulkan` and `ICED_PRESENT_MODE=mailbox` before exec. Env vars
  use `${VAR:-default}` so users can override. Previously, binaries launched with
  no GPU backend preference, falling back to wgpu auto-detection.

- **Nix: fixed Vulkan ICD discovery** — Removed broken `VK_ICD_FILENAMES` from
  devshell that pointed to `pkgs.vulkan-loader` (which has no ICD files). The
  Vulkan loader automatically discovers ICDs from `/run/opengl-driver/` on NixOS
  via `hardware.graphics.enable`. The old path silently disabled ICD discovery.

### Fixed

- **USB: multi-stick metadata resolution** — When multiple USB sticks were
  connected, switching between playlists from different sticks could load tracks
  with wrong metadata (missing beatgrid, default 120 BPM, no key). Root cause:
  `load_track_metadata()` only checked the "active" USB database, ignoring other
  mounted sticks. Now resolves the correct database from the track's path itself
  via `find_collection_root()` + the centralized USB database cache, making
  metadata lookup independent of which stick is currently browsed. Also fixed the
  browser storage sync guard that prevented USB→USB switches between sticks.

- **USB: export progress and performance** — Pressing "Export" showed no UI feedback
  for metadata-only changes (no progress bar, export button stayed clickable). The UI
  now transitions immediately when export starts, and the progress bar correctly counts
  metadata-only updates. Also eliminated an expensive database re-open on USB flash
  after export completes (lazy cache invalidation instead).

- **USB: sync plan performance** — "Calculating changes" in the export modal took
  60+ seconds for a 200-track collection because supplementary metadata (cue points,
  saved loops, stem links, ML analysis, tags, audio features) was fetched with 6
  individual database queries per track — over 2,400 sequential round trips on USB
  flash storage. Replaced with 6 bulk parameterless queries that fetch all rows in
  a single pass and group by track ID in Rust, reducing scan time to ~1-2 seconds.

- **USB: device label resolution** — USB sticks were showing kernel device names
  (e.g. "/dev/sda") instead of human-readable names. On Linux, now resolves the
  filesystem label from `/dev/disk/by-label/`, falling back to the hardware model
  name from sysfs (e.g. "STORE N GO"), then `/dev/sdX` as last resort. macOS and
  Windows are unaffected (sysinfo already returns proper volume labels there).

- **Embedded: ES8388 audio init** — `mesh-audio-init` service was failing on every
  boot because the `Headphone` mixer control is a switch, not a volume. Replaced
  the single broken `amixer` command with a proper init script that enables the
  headphone amplifier path (`hp switch` on), sets PCM and output volumes, disables
  3D spatial processing, and ensures left/right mixer paths are enabled.
- **Embedded: ALSA device aliases** — `mesh_cue` and `mesh_master` PCM aliases
  used `type hw` (raw hardware access), which rejected mono audio and any format
  the ES8388 doesn't natively accept. Changed to `type plug` with nested
  `slave.pcm` for automatic format, channel, and sample rate conversion.
- **Embedded: PipeWire low-latency config** — Added PipeWire clock configuration
  with 256-sample quantum at 48kHz (5.33ms per period), min 64, max 1024. Without
  this, PipeWire defaulted to 1024 samples (21.3ms).
- **Embedded: WirePlumber device rules** — Split the combined ES8388/PCM5102A
  match into separate rules with per-device priorities. Reduced `api.alsa.headroom`
  from 256 to 0 (I2S codecs use DMA, not USB batch transfer, so headroom adds
  unnecessary latency). PCM5102A gets higher `priority.driver` so it becomes the
  graph clock source when connected.
- **Embedded: JACK audio routing via pw-link** — PipeWire JACK clients with
  `node.always-process=true` (set by the JACK layer) remain on Dummy-Driver
  unless explicit port links exist to a real ALSA sink. `target.object`,
  `PIPEWIRE_NODE`, and `priority.driver` all proved insufficient — they are
  routing hints that don't force driver assignment. The kiosk wrapper now starts
  mesh-player via `pw-jack` in the background, waits for its JACK ports to
  register, then creates `pw-link` connections from `master_left`/`master_right`
  to the ES8388's `playback_FL`/`playback_FR`. This reliably moves mesh-player
  off Dummy-Driver onto the ES8388 graph driver.
- **Embedded: WirePlumber config via environment.etc** — The NixOS
  `services.pipewire.wireplumber.extraConfig` option silently fails to create
  config files on NixOS 24.11 (`/etc/wireplumber/` was empty). Switched to
  `environment.etc` for direct file creation with WirePlumber 0.5 SPA-JSON
  format, ensuring ALSA tuning rules (`session.suspend-timeout-seconds`,
  `api.alsa.period-size`, `priority.driver`) are actually deployed.
- **CI: Windows cross-compilation bindgen** — `signalsmith-stretch` bindgen
  failed with `stdbool.h` not found after Phase 4's `unset BINDGEN_EXTRA_CLANG_ARGS`.
  bindgen auto-injects `--target=x86_64-pc-windows-gnu` which makes clang lose
  its resource directory. Now re-exports `BINDGEN_EXTRA_CLANG_ARGS` with just
  the clang include path (no MinGW sysroot) immediately after the unset.
- **Embedded: mesh-player logging** — Process substitution (`> >(systemd-cat ...)`)
  doesn't survive through `pw-jack`'s exec chain, so all mesh-player log output
  was silently lost. Replaced with a named FIFO pipe to `systemd-cat`, making
  logs available via `journalctl -t mesh-player`. Also sets `RUST_LOG=info` by
  default (overridable via `systemctl set-environment`).

### Added

- **MIDI: Master BPM slider control** — The master BPM slider is now controllable
  via MIDI. `GlobalAction::SetBpm` was stubbed out; it now routes through the
  full pipeline: `range_for_action("global.bpm")` maps CC 0-127 to 60-200 BPM,
  the mapping engine converts to `SetBpm`, and the app handler calls
  `set_global_bpm_with_engine()`. The MIDI learn wizard includes a "Move the
  BPM slider" step at the end of the Browser phase across all layout variants.
- **USB: Set filesystem label during export** — When exporting to a USB device,
  a new "Label" text input lets you set a custom filesystem label (e.g. "Mesh DJ").
  Tries `FS_IOC_SETFSLABEL` ioctl first (works on mounted ext4/btrfs/xfs, and FAT on
  kernel 7.0+). Falls back to udisks2 D-Bus `SetLabel` (works for regular users on
  removable devices via polkit, no root needed). Pre-fills with the device's current
  label; shows filesystem-specific max length hints. Label setting is non-fatal —
  failure is logged but doesn't abort the export.
- **Embedded: Default config files** — Ship `midi.yaml`, `slicer-presets.yaml`,
  and `theme.yaml` to `/home/mesh/Music/mesh-collection/` via systemd tmpfiles
  `C` (copy-if-not-exists) rules, so the Orange Pi boots with working defaults
  while preserving any user modifications on subsequent updates.
- **Embedded: PAM audio limits** — `@audio` group gets unlimited memlock,
  rtprio 99, and nice -19 for real-time audio scheduling.
- **Embedded: RT kernel tuning** — Added `threadirqs` kernel parameter (threads
  all IRQ handlers for priority control) and `vm.swappiness=10` (keeps audio
  buffers in RAM).

### Changed

- **CI: Split native deps cache from Rust build cache** — Essentia, FFmpeg, and
  TagLib (pinned C/C++ libraries that never change) now have a separate GitHub
  Actions cache keyed on the build script hash instead of `Cargo.lock`. Previously,
  any Rust dependency update invalidated the entire cache, forcing 10-60 minute
  rebuilds of unchanged native libraries. The stable deps cache persists across
  Cargo.lock changes, saving significant CI time on every release.
- **CI: Regenerated binary cache signing key** — Replaced the cache signing key
  pair and fixed the narinfo signing pipeline. `nix copy` and `nix store sign`
  are now separate steps with key format validation and post-sign verification
  to prevent silent signing failures.

---

## [0.9.2]

### Added

- **In-app WiFi management** — Settings now include a Network section with WiFi
  scanning, connection, and disconnection. Uses `nmrs` (Rust D-Bus bindings for
  NetworkManager) instead of shell-based `nmcli` for type-safe, reliable network
  operations. Each D-Bus call runs on a dedicated thread with its own
  single-threaded tokio runtime to work around nmrs's `!Send` futures and iced's
  nested-runtime constraint. Secured networks open an on-screen keyboard for
  password entry. The Cancel button is part of the key grid (after Done) so it's
  reachable via MIDI encoder navigation. Keys with distinct shifted symbols
  (numbers, punctuation) show a small dark-gray hint in the bottom-right corner
  so users know which symbols are available via Shift without guessing.
  Platform-gated: Linux-only via `#[cfg(target_os = "linux")]` with no-op stubs
  on other platforms, so Windows builds are unaffected. The on-screen keyboard
  widget lives in mesh-widgets for reuse across crates.
- **OTA system updates** — New System Update section in settings checks GitHub
  releases for newer versions, installs via the `mesh-update` systemd service,
  shows live journal output during installation, and restarts the cage compositor
  to run the new binary. Only active on NixOS embedded (detected by `/etc/NIXOS`).
- **MIDI settings navigation** — New `global.settings_toggle` action
  opens/closes the settings modal via MIDI. When open, the browser encoder
  scrolls through settings, encoder press enters editing mode for the focused
  setting, and scroll cycles through options with live draft preview. Closing
  auto-saves if changes were made. Opening settings automatically forces browse
  mode on the mapping engine so encoders that share loop-size and browser-scroll
  mappings (mode-switched) produce browser events for navigation. Previous
  browse mode state is saved and restored on close. The settings scrollable
  auto-scrolls to keep the focused setting visible as the encoder moves through
  the list. Audio device dropdowns expand into inline button groups during
  editing mode so all options are visible while cycling with the encoder.
- **MIDI sub-panel navigation** — When MIDI-navigating to the Network or System
  Update entries in settings, pressing the encoder enters a domain-specific
  sub-panel directly (no editing-mode step). WiFi sub-panel: encoder cycles
  through scanned networks, press connects (or opens keyboard for secured
  networks). Update sub-panel: encoder cycles between Check and Install/Restart
  actions with visual highlighting on the focused action. Shift+encoder press
  steps out of the current mode (sub-panel → scroll). The MIDI Learn section
  is now a navigable entry — encoder press triggers Start MIDI Learn directly.
  Priority chain: keyboard > sub-panel > settings edit > settings scroll >
  normal MIDI.
- **Embedded: silent boot** — Comprehensive kernel param and systemd
  configuration for minimal boot output: `loglevel=0`, `quiet`,
  `rd.systemd.show_status=false`, `systemd.show_status=false`,
  `rd.udev.log_level=3`, `kernel.printk=0 0 0 0`, `vt.global_cursor_default=0`,
  `logo.nologo`. Replaces the previous Plymouth-based splash which failed to
  render the custom script theme on ARM/RK3588S (fell back to NixOS default).
- **Embedded: NetworkManager permissions** — mesh user added to
  `networkmanager` group, polkit rules expanded to allow managing both
  `mesh-update.service` and `cage-tty1.service`.

---

## [0.9.1]

### Fixed

- **Embedded: mesh-player crash on boot (`NoWaylandLib`)** — PipeWire's PAM
  session overrides the systemd `Environment=` directive, clobbering
  `LD_LIBRARY_PATH` with only `pipewire-jack/lib`. winit/wgpu `dlopen()` calls
  for `libwayland-client.so`, `libxkbcommon.so`, `libEGL.so`, and
  `libvulkan.so` then fail. Fixed with a wrapper script that sets
  `LD_LIBRARY_PATH` before exec'ing mesh-player, immune to PAM overrides.

### Added

- **Embedded: USB automounting** — udev rules auto-mount USB sticks to
  `/media/<label>` via `systemd-mount` when plugged in, and clean up on removal.
  No daemon, no D-Bus session, no polkit required — runs directly from udev
  context. Mounted with `noatime` to reduce background writes and make
  hot-unplug safer. mesh-player detects new mounts via its existing 2-second
  `sysinfo` polling loop.
- **Embedded: debugging infrastructure** — cage `-s` flag enables VT switching
  (Ctrl+Alt+F2), TTY2 getty provides a login shell for local debugging,
  persistent journal (`Storage=persistent`, 50MB cap) preserves logs across
  reboots, and `boot.initrd.systemd.emergencyAccess` enables emergency shell
  access during boot failures.
- **Windows cross-compilation failing on `stdbool.h`** — The container-based
  Windows build (`build-windows.nix`) set `BINDGEN_EXTRA_CLANG_ARGS` with
  `--sysroot=/usr/x86_64-w64-mingw32` for Essentia's cross-compilation, but
  forgot to unset it before building mesh-player. This caused clang to search
  the MinGW sysroot for compiler built-in headers like `stdbool.h`, which
  aren't there — they live in clang's resource directory. Fixed by unsetting
  `BINDGEN_EXTRA_CLANG_ARGS` before Phase 4 (mesh-player) and re-exporting it
  with the clang resource directory explicitly included before Phase 5
  (mesh-cue).

---

## [0.9.0]

### Added

- **Metadata parsing for import pipeline** — Artist and title extraction now
  reads embedded audio tags (ID3v2, Vorbis, MP4, FLAC) via lofty before falling
  back to filename parsing. The filename parser handles UVR5 numeric prefixes
  (`56_Artist - Title`), track number prefixes (`01 - `), en/em dashes,
  underscore separators, and multi-dash filenames with DB-assisted known-artist
  disambiguation (`Black Sun Empire - Arrakis - Remix` correctly splits on the
  artist). Artist connectors (`&`, `feat.`, `ft.`, `x`, `vs.`) are normalized
  to comma-separated lists, square brackets are converted to parentheses, and
  `(Original Mix)` is stripped. The known-artist set is loaded once per batch
  from the database for case-insensitive matching.
- **Suggestion energy slider in MIDI learn** — The Browser phase now includes
  two Suggestion Energy steps (left and right side), placed after the browse
  encoder. This allows mapping physical knobs/faders to the smart suggestion
  energy direction slider during MIDI learn. The `deck.suggestion_energy` action
  controls the global energy bias (DROP ↔ PEAK) used by the suggestion engine.
- **Streaming track loading with priority regions** — Track loading is now a
  three-phase progressive pipeline: (1) skeleton with metadata loads instantly
  (<10 ms), giving immediate access to beat markers, cue markers, and navigation;
  (2) priority regions around hot cues and the drop marker load next (~200 ms);
  (3) remaining audio fills in incrementally. The DJ can beat-jump, seek, and
  navigate cue points while audio loads in the background.
- **Incremental waveform visualization** — The overview waveform now grows
  visually as audio loads. Priority regions (hot cue areas) appear first, then
  gap regions fill in progressively in ~15-second visual batches. Unloaded areas
  render as flat/silent, giving clear visual feedback of which parts of the track
  are ready for playback. High-resolution zoomed peaks also update incrementally.
- **Instant partial playback during loading** — Stem buffer snapshots are
  delivered to the audio engine at ~100-second intervals via `UpgradeStems`, so
  the DJ can press play or cue and hear audio from any loaded region. Visual
  peak updates (cheap, ~2 MB) are decoupled from stem clones (expensive,
  ~460 MB) — the waveform grows smoothly while playback catches up at clone
  boundaries. Unloaded areas produce silence on playback.
- **Region-based audio file reading** — New `read_region_into()` method on
  `AudioFileReader` enables seeking to arbitrary sample positions and reading
  directly into pre-allocated stem buffers. Supports 16-bit, 24-bit, and
  32-bit (float and integer) formats. Existing full-read methods now delegate
  to the region reader internally, eliminating code duplication.
- **Engine `UpgradeStems` command** — New real-time-safe command that upgrades
  a deck's stem buffers without resetting playback position. Uses `basedrop::Shared`
  for lock-free deallocation on the audio thread.
- **Skeleton track loading** — `create_skeleton_and_load()` on the domain layer
  creates an instant-load track with zero-length stems but correct duration,
  beat grid, cue points, and metadata. The engine uses `duration_samples` for
  navigation and `stem_data.len()` for audio reads, so navigation works
  immediately while stems are still empty.
- **Original filename preservation** — The raw filename (`base_name`) is now
  saved as `original_name` in the tracks database before metadata parsing
  normalizes it into artist/title. This enables re-running metadata analysis
  later (e.g., after parser improvements) without reimporting. Existing
  databases are migrated automatically on startup.
- **Reanalysis overhaul** — The context menu now offers two reanalysis actions
  instead of five: "Re-analyse Beats" fires immediately (unchanged BPM/beat
  grid pipeline), while "Re-analyse Metadata..." opens a modal with four
  checkboxes (all enabled by default): Name/Artist (re-parse `original_name`),
  Loudness (LUFS via Essentia subprocess), Key (key detection via Essentia),
  and Tags (genre, mood, vocal detection via EffNet ML pipeline). Only the
  ticked analyses run, and Essentia subprocess calls are batched when both
  loudness and key are selected. Beat analysis is kept separate since beat
  grids are frequently edited manually.
- **DB schema migration for tracks** — Automatic migration detects old track
  schemas missing the `original_name` column. Data is backed up, the relation
  is recreated with the new schema, and all rows are restored with
  `original_name` defaulted to empty string. Runs transparently on startup.

### Improved

- **Drop-aware LUFS measurement** — Loudness analysis now targets the loudest
  sections of a track (drops, refrains) instead of the whole-track average, so
  auto-gain matches tracks where it matters. Requires LUFS reanalysis.
- **Suggestion energy MIDI debounce** — The energy direction fader now uses
  trailing-edge debounce (300ms) so the suggestion query only fires once the
  fader stops moving, instead of on every value change. Moving the fader also
  auto-enables suggestion mode if not already active.
- **Track load memory usage** — Stem clones (~460 MB each) are sent only at
  ~100-second intervals (~5 clones per 5-minute track, ~500 ms total overhead).
  Visual peak updates are sent every ~15 seconds at negligible cost (~2 MB).
  Peak memory during loading is ~920 MB; the `basedrop` GC thread collects
  stale clones within 100 ms, preventing unbounded growth.
- **Priority region planning** — New `regions` module computes optimal load
  regions around hot cues, drop markers, and the first beat. Regions within
  64 beats of each other are merged to minimize seek operations. Gap regions
  (everything not covered by priority areas) are computed for sequential
  background filling.

### Fixed

- **Embedded SD image not booting on Orange Pi 5** — The SD card image built by
  CI was missing the U-Boot bootloader. The upstream `gnull/nixos-rk3588` Orange
  Pi 5 module does not embed U-Boot (it expects SPI NOR flash to be
  pre-programmed). Added prebuilt U-Boot binaries (idbloader.img + u-boot.itb,
  extracted from official Orange Pi Debian v1.1.8) and `postBuildCommands` that
  `dd` them into the image gap at the Rockchip-mandated sector offsets (64 and
  16384). The image now boots on a factory-fresh board with no prior setup.
- **Board name mismatch** — All references to "Orange Pi 5 Pro" corrected to
  "Orange Pi 5" across flake.nix, CI workflows, devshell, and flash script.
  The target board is the base Orange Pi 5 (RK3588S).

---

## [0.8.10]

### Improved

- **USB export throughput** — Rewrote the export pipeline to separate file I/O
  from database I/O. Track files are now copied sequentially with 1 MB buffered
  writes and `fsync` per file (replacing parallel random writes via `par_iter`).
  The USB database is staged locally: copied to a temp directory, updated there
  with all metadata/playlist/deletion operations, then written back as a single
  sequential copy. This eliminates random I/O on flash storage and should reduce
  export times by 50–70%.
- **Batched tag inserts** — `sync_track_atomic` now uses a single CozoDB batch
  query for tag insertion instead of N individual `:put` operations per track.
- **Buffered file copy with fsync** — New `copy_large_file()` utility uses
  `BufReader`/`BufWriter` with 1 MB buffers, `posix_fadvise(SEQUENTIAL)` on
  Linux, and `sync_all()` for data safety on removable media.
- **Simplified export progress** — Merged five separate metadata/playlist
  progress phases into a single unified "Updating database" phase, reducing UI
  complexity and message overhead.

---

## [0.8.9]

### Fixed

- **Browser not updating during import/reanalysis** — Track metadata (BPM, key,
  tags, new tracks) now refreshes in real-time as each track completes instead
  of requiring manual navigation. The tick handler was silently discarding all
  `Task` returns from progress handlers; these are now collected and returned
  via `Task::batch()`. Reanalysis also fires per-track `RefreshCollection` on
  success, matching the pattern import already used.
- **Audio muted during USB export** — Removed unnecessary audio stream
  pause/resume around USB export. Only import and reanalysis (which are
  CPU-intensive) pause the stream; export is I/O-bound and doesn't need it.
- **Tags column too wide in mesh-cue** — Reduced track table Tags column from
  300px to 150px so the Name column has more room.
- **mesh-cue Windows build failing** — mesh-cue hardcoded `pd-effects` as a
  direct dependency, pulling in `libffi-sys` which fails to cross-compile for
  MinGW. Now feature-gated like mesh-player: `pd-effects` is a default feature
  (enabled on Linux) but disabled by `--no-default-features` on Windows. The
  PD stub module was also updated with missing methods/fields. Windows build
  script now fails on either crate instead of silently skipping mesh-cue.
- **mesh-cue Windows linker error** — `build.rs` emitted ELF-specific linker
  flags (`--disable-new-dtags`, `--no-as-needed`, `-rpath`) unconditionally.
  MinGW's `ld` doesn't recognize these. Now gated behind a `TARGET` check so
  they only apply on Linux.

---

## [0.8.8]

### Fixed

- **Audio crackling during batch operations** — CPAL audio stream is now paused
  during import, export, and reanalysis to eliminate buffer underruns caused by
  CPU contention between the real-time audio callback and heavy processing
  threads (ML inference, stem separation, file I/O). The stream starts paused
  at launch (no track loaded = no audio needed) and resumes only when a track
  is loaded for preview. Cancel and error paths also correctly resume audio.

---

## [0.8.7]

### Added

- **Cross-source suggestions** — Suggestions now query all connected databases
  (local + USB). HNSW vector search runs across both sources, combining results
  into a unified ranked list with source tags ("Local" / "USB") on each
  suggestion.
- **Cross-source deduplication** — When the same track exists in both local and
  USB databases, only the entry with the best HNSW distance is kept, preventing
  duplicate suggestions.
- **USB export: tags, ML analysis, audio features & presets** — USB export now
  syncs ML analysis data, track tags, and audio feature vectors alongside track
  files. Effect presets (stems, decks, slicer) are also copied to USB.
- **Metadata sync progress** — USB export reports per-track progress during the
  metadata-only sync phase, keeping the overlay progress bar responsive.
- **ML audio analysis** — 6 new EffNet classification heads: timbre
  (bright/dark), tonal/atonal, acoustic, electronic, danceability, and
  approachability. ML-based vocal detection replaces RMS-based approach.
- **Energy-direction suggestion scoring** — Suggestions incorporate ML arousal
  scores, genre-normalized aggression, and production match scoring. Key scoring
  blends toward energy direction at fader extremes.
- **Event-driven seed refresh** — Suggestion seeds now auto-refresh on deck
  load, play/pause, and volume changes with debounced timer.
- **Multi-factor reason tags** — Suggestion entries show sorted reason tags
  (key compatibility, energy direction) with color-coded confidence.
- **Hierarchical USB playlists** — USB export supports nested playlist folders
  with portable relative paths.

### Fixed

- **Audio features not exported to USB** — `get_audio_features()` failed
  silently on CozoDB's `DataValue::Vec(Vector::F32(...))` type, only matching
  `DataValue::List`. Audio feature vectors were never synced to USB.
- **Cross-DB HNSW search** — `find_similar_by_vector()` passed the query vector
  as `DataValue::List`, but HNSW requires a proper Vector type. Fixed with
  CozoScript's `vec()` function.
- **USB track metadata lookup** — `load_track_metadata()` now converts absolute
  paths to relative paths for USB storage.
- **USB playlist browsing** — Fixed playlist browsing broken after relative-path
  migration.
- **Track deletion cleanup** — `delete_track` now cleans all child relations
  (cue points, saved loops, stem links, tags, ML analysis, audio features).
- **Export phase message ordering** — Corrected progress message ordering to
  prevent UI stall during export.
- **DnB sub-genre consolidation** — Consolidated DnB sub-genre tags, suppressed
  redundant Instrumental genre tag.

### Improved

- **Tick handler performance** — Optimized hot-path tick handler with
  documentation for lock-free architecture.
- **Export metadata sync performance** — Replaced O(n^2) per-track
  `get_all_tracks()` scan with pre-built `HashMap` lookup.
