If unsure ask questions, dont assume stuff. Try to make sure that logic lies
as much as possible in mesh-core and mesh-widget and only if necessary in the ui.

# Features

## Collection Browser
- [ ] Tag editing UI: support adding, removing, and editing tags on tracks
  directly from the browser. Needs inline tag editor or context menu with
  autocomplete from existing tags (get_all_tags). Color picker optional.

## MIDI
- [ ] Jog wheel beat nudging for backwards compatibility with older devices
  (like SB2). Must work with current snapping system: when a user nudges by N
  samples, that offset is stored for this deck only, resets on load of a new
  track and is preserved across beat jumps, hot cue presses and
  other seek operations so the DJ doesn't need to nudge again. We could
  potentially map several possible midi interfaces to this: classical jog wheel,
  +/- buttons for fine grained nudging.

## Slicer
- [ ] Single morph knob per deck that scrolls through preset banks (up to 8
  presets per bank).

## B2B Mode
- i would like to support a b2b mode, where 2 people with mesh systems, can
  connect their 2 systems together via ethernet lan cable, the system recognizes
  this and automatically goes into b2b mode, where each of their system shows
  the info of the other deck (waveforms). this means that each dj has their own
  hardware and settings, but can also play music from their partners library and
  everything is synced to the same master clock and bpm. all playlist features
  like smart suggestions should also work.

## Smart Suggestions (v3 — Future Improvements)

V2 (current) uses a unified scoring formula with energy direction fader and
selectable key scoring model (Camelot / Krumhansl). The following are ideas
for v3 and beyond.

### History-Informed Suggestions (requires deep research)
- [ ] **Play history graph**: once mesh-player records session history to the
  USB stick (see DJ History below), feed co-play data into the suggestion
  algorithm via the graph DB. Tracks that have been played together frequently
  across sessions should score higher as candidates — they have proven
  real-world compatibility that audio features alone cannot capture.
- [ ] **Pattern mining from play history**: research existing algorithms and
  systems for finding patterns in DJ play histories. Areas to investigate:
  - Collaborative filtering (item-item similarity from co-occurrence matrices)
  - Sequential pattern mining (frequent subsequences in set tracklists)
  - Graph-based recommendations (PageRank / random walks on the track
    co-play graph, weighted by recency and frequency)
  - Transition probability models (Markov chains over track-to-track
    transitions, conditioned on energy/key context)
  - Session-aware recommendation systems from the RecSys literature
    (e.g. GRU4Rec, STAMP, SR-GNN — adapted for DJ set context)
  - Existing DJ-specific research: DJ mix graph datasets, automatic
    playlist continuation, set.fm / 1001tracklists data analysis
  This needs a dedicated research document before implementation.
- [ ] **Negative signals**: tracks that were loaded but immediately skipped
  (played < 30 seconds) could receive a soft penalty in future sessions,
  especially when paired with the same seed tracks.
- [ ] **DJ profile divergence**: when multiple DJs use the same collection
  (B2B, shared USB), per-DJ history should be kept separate so suggestions
  reflect each DJ's mixing style, not a blended average.


### Infrastructure

## DJ History & Playlists
- [ ] Keep DJ history per session, per DJ, persisted to DB while playing.
  Initially used to update graph-based relations for track exploration, later
  for full set reconstruction.
- [ ] Improved playlist features using graph DB and vector features:
  - [ ] Similarity search for tracks or stems (smart dynamic playlist matching
    vibe of currently running track or a single stem).
  - [ ] Dynamic smart playlists based on Camelot key with energy options
    (lower / keep / raise energy).
  - [ ] Database backup (without wav files, just DB).

## Audio Processing
- [ ] Live peak meter per channel and master channel.
- [ ] Set recording master output.
- [ ] Real-time short-term LUFS normalisation per stem (no latency) using
  essentia, ebur128 or lufs crate. Goal: stems after FX processing should be
  comparable loudness to input stem loudness, since processing can be very
  loud or silent.
- [ ] Built-in native effects (beat-synced echo, flanger, phaser, gater, etc.).

## Documentation
- we need much better strucutred and complete documentation:
  * a readme that is actually useful for people that come to github, see the
    project for the first time and dont want to get overwhelmed with
    information. we need a strong overview, which is still faithful with the
    goal of mesh. the key selling point of mesh is its very easy for beginners
    to use (due to beat sync, auto lufs gain control etc), but has a ton of features that do not exist on denon, pioneer, other
    software based mixing software like traktor etc (actually research what all
    these have, dont make false claims), also explain that due to inherent
    workflow of autosync and grid analysis electronic music and 4/4 beats are
    the target music, then a short list of the most important
    high-level features, which are important for the overall workflow of
    preparation (cue) and performance(player). check the previous list, but most importantly actually scan the codebase to see if anything is missing or not true. then a roadmap, which lists features that we still need to implement and dont have(again check the todo file, this also should just describe the high level important features) then a how to install, that references the build artifacts. Remove the contributing section, i dont want any contributions. But people are welcome to start issues if they have feature requests or bugs (we need templates for this, research good templates). Keep License and Acknoledgements and extend these if necessary.
  * linked md docs for various important documents:
    + the mesh-collection folder, where it is for each os, how to import, how to
      export to usb stick etc.
    + midi/hid mapping, already supported devices
    + effects (especially how to bundle puredata effets, including nn~ external,
      or generally externals, but nn is special since we need to build this),
      clap effects etc, where to place them
    + Embedded (we already have this), including a complete bom for recommended
      setup, how to wire, how to install sd image(we have the nix cache now,
      reference this).

# Bugs
- [ ] When deleting a file in the file browser, select the next item (or
  previous if no next) instead of scrolling to the top ( i think this happens,
  since we index something that isnt there anymore ).
- [ ] USB manager should invalidate DB connection and cache and notify UI to
  return to the hierarchy one above in file browser when a USB stick is removed.
  Currently the user can still scroll and "load" tracks from the unexisting usb
  stick.

# Performance
- [ ] Optimise stem storage (currently ~200-300 MB per multi-track file).

# Open Questions
- [ ] B2B settings management: when multiple DJs play on the same device, each
  with their own USB stick and settings, how does the system decide which
  settings to use? First connected? Should there be a B2B mode where specific
  decks use specific DJ's settings?

# Auto Headphones Cue system
- [ ] Instead of the user needing to automatically cue tracks to headphones out
  (theres a button per channel currently), we can automate this. usually the use
  case for this button is to pre listen to a newly loaded track, to beat match
  and check if this fits and we are at the correct beat grid offset etc, but
  since we have auto sync, a lot of this is useless, and for mesh its only
  important to prelisten to the newly cued track to verify its on the same
  beatgrid snap and that it musically fits. this means that all tracks, that are
  loaded but currently at volume 0 (or a threshold very low, like 0.3 of 1)
  should be send to cue headphones out. this should not just binary, but
  gradually mixed in so at 0.5 they might be audible still a bit, at 0.3 fully (at 0 still fully )(exponential curve or just two stages linear, both is fine). users can still
  use the cue buttons (make this configurable in the player ui, default is
  auto-cue). Autocue should ONLY be active, when the master and cue outputs are
  different outputs, otherwise there is problems with the output! plan this thorougly, break it into subtasks, at the end document
  this as a new feature in the changelog and commit all.

# DB
- [ ] we need to introduce database versions, so when the schema changes the db
  knows it needs to migrate. this is also relevant for usb sticks, at some point
  we need to be able to be backwards compatible (not yet, but we should pave the
  way for this possibility, by adding a versioning system for the schema/db version)

# UPDATE LIFECYCLE
- [ ] connecting to wifi should first check if we already have credentials
  stored (via networkmanager) and just reconnect then, since we wont need a
  password entry then.

## Embedded: Silent Boot (investigated, partially working)
- [x] Removed Plymouth splash entirely — the script theme (`ModuleName=script`)
  failed on ARM/RK3588S. Plymouth rendered the NixOS fallback theme instead of
  the custom mesh theme. Root cause never fully identified, but likely:
  font rendering (`Image.Text()` with "Sans Bold 48" needs fonts in initrd),
  and/or `rd.systemd.show_status=auto` switching to verbose mode on slow SD I/O.
  Armbian/Orange Pi Debian doesn't use Plymouth at all — they use a U-Boot-level
  boot logo via the proprietary `resource.img` mechanism.
- [x] Silent boot params applied: `quiet`, `loglevel=0`,
  `rd.systemd.show_status=false`, `systemd.show_status=false`,
  `udev.log_level=3`, `rd.udev.log_level=3`, `vt.global_cursor_default=0`,
  `logo.nologo`, `kernel.printk=0 0 0 0`. Significantly reduces boot output
  but does not achieve fully silent boot on this hardware.
- Attempted `console=tty2` — redirects ALL console output to tty2, but this
  breaks cage-tty1 because the kernel's active VT switches to tty2, preventing
  cage from acquiring DRM master on tty1. Not usable with kiosk setups.
- Future option: raw framebuffer `dd` service (write pre-rendered .fb to
  /dev/fb0 early in boot) or recompile vendor U-Boot with `CONFIG_SPLASH_SCREEN`
  for a true pre-kernel splash.

# OTHER
- [ ] slicer presets do not get triggered from the midi mapped f1. check 1.
  the mapping file, 2. the mapping in the app and give me a report of why this
  might happen. the current mapping file is in momentary mode. also the visuals
  of slicer mode with the new shader does not align with what we had with the
  canvas before, slicer mode has a fixed window size of 16 beats and the plahead
  moves instead of the waveform. it also seems to be not connected to the engine
  anymore, the engine should still support slicer mode.
- [ ] when starting the player first, then connecting the hid and midi devices, they
  are not recognized by mesh-player, we already have reconnection logic (connecting then
  disconnecting hardware works well), reuse that for detecting hardware after
  the software launch. we know which hardware to expect from the midi mapping
  file.
- [ ] some tracks still have some numbers in front of the name (as part of the
  artist apparently) from the name parsing, we need to fix that, some examples:
  * 01 Black Sun Empire - Feed The Machine (you can check the original name in
    /home/data01/Music/mesh-collection/import/backup/)
- [ ] Overhaul midi mapping mode to be a tree like hierarchy with some questions at each parent node to figure out the style of mapping we want, instead of a list with
  questions at the front.


- Iced cusomization options. Interesting is the settings, search what else we
  can set there, which might be relevant for us. Also theming is very
  interesting, we havent looked into that yet, research how theming works with
  iced. we should also set a title in each binary:
     * run() -- Runs the application
     * settings(Settings) -- Sets the iced::Settings
     * antialiasing(bool) -- Enables/disables antialiasing
     * default_font(Font) -- Sets the default font
     * font(impl Into<Cow<'static, [u8]>>) -- Adds a custom font
     * scale_factor(impl Fn(&State) -> f64) -- Custom scale factor logic
     * window(window::Settings) -- Sets window settings
     * centered() -- Centers the window
     * window_size(Size) -- Sets window dimensions
     * transparent(bool) -- Window transparency
     * resizable(bool) -- Window resizability
     * decorations(bool) -- Window decorations
     * position(Position) -- Window position
     * level(Level) -- Window level (e.g., always-on-top)
     * exit_on_close_request(bool) -- Controls exit behavior
     * title(impl Fn(&State) -> String) -- Dynamic title
     * subscription(impl Fn(&State) -> Subscription<Message>) -- Subscriptions
     * theme(impl Fn(&State) -> Theme) -- Theme function
     * style(impl Fn(&State, &Theme) -> Color) -- Style logic
     * executor() -- Executor type
