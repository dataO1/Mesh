If unsure ask questions, dont assume stuff. Try to make sure that logic lies
as much as possible in mesh-core and mesh-widget and only if necessary in the ui.

# Features

## Collection Browser
- [ ] Tag editing UI: support adding, removing, and editing tags on tracks
  directly from the browser. Needs inline tag editor or context menu with
  autocomplete from existing tags (get_all_tags). Color picker optional.

## MIDI
- [ ] optional: Jog wheel beat nudging for backwards compatibility with older devices
  (like SB2). Must work with current snapping system: when a user nudges by N
  samples, that offset is stored for this deck only, resets on load of a new
  track and is preserved across beat jumps, hot cue presses and
  other seek operations so the DJ doesn't need to nudge again. We could
  potentially map several possible midi interfaces to this: classical jog wheel,
  +/- buttons for fine grained nudging.

## Slicer
- [ ] optional: Single morph knob per deck that scrolls through preset banks (up to 8
  presets per bank).

## B2B Mode
- big future update, after v1.0.0: i would like to support a b2b mode, where 2 people with mesh systems, can
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
- [x] **Session history foundation**: HistoryManager records all play data
  (loads, plays, hot cues, loops, co-play, suggestion metadata) to all active
  databases. Played tracks excluded from suggestions and dimmed in browser.
- [ ] **Play history graph**: feed co-play data into the suggestion algorithm
  via the graph DB. Tracks that have been played together frequently across
  sessions should score higher as candidates — they have proven real-world
  compatibility that audio features alone cannot capture. Time-decayed
  co-play scoring (half-life ~30 days) to prioritize recent patterns.
- [ ] **Pattern mining from play history**: research existing algorithms for
  finding patterns in DJ play histories (see DJ History section for details).
  Areas: collaborative filtering, PrefixSpan, graph-based recommendations,
  transition probability models, session-aware RecSys (GRU4Rec, SR-GNN).
- [ ] **Negative signals**: tracks loaded but played < 30s could receive a
  soft penalty in future sessions, especially when paired with the same seeds.
- [ ] **DJ profile divergence**: see DJ History section.


### Infrastructure

## DJ History & Playlists
- [x] Keep DJ history per session, persisted to all active DBs while playing.
  HistoryManager tracks loads, plays, hot cues, loops, co-play (played_with),
  suggestion metadata. Played tracks dimmed in browser and excluded from
  suggestions.
- [ ] **Set reconstruction UI**: session browser that lists past sessions with
  timeline view showing which tracks played on which decks, when transitions
  happened (via played_with), and how long each track was played. Export as
  text tracklist or shareable format.
- [ ] **Track metadata mining from play history**: once enough sessions are
  recorded (50+), mine patterns to improve suggestions:
  - Co-play frequency scoring: tracks played together often across sessions
    get a bonus in suggestion scoring (time-decayed, half-life ~30 days)
  - Transition probability models: Markov chains over track-to-track
    transitions, conditioned on energy/key context
  - Skip penalty: tracks loaded but played < 30s get a soft penalty when
    paired with the same seed tracks in future sessions
  - PrefixSpan sequential pattern mining for finding common subsequences
    in set tracklists (requires 200+ sessions for meaningful patterns)
- [ ] **Per-DJ history divergence**: when multiple DJs use the same collection
  (B2B, shared USB), per-DJ history (keyed by USB stick identity) should be
  kept separate so suggestions reflect each DJ's mixing style.
- [ ] Improved playlist features using graph DB and vector features.
- [ ] Database backup (without wav files, just DB, so hot cues, markers,
  analysis data etc.).

## Audio Processing
- [ ] Live peak meter per channel and master channel.
- [ ] Set recording master output.
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

# Performance
- [ ] potential GPU performance optimisation: we can save even more gpu power by making the overview waveform a canvas
  again (we already have some working deprecated code for that reuse it), but
  render high refresh needed parts in the shader (only the playhead position).
  everything else can then use smart messaging and only update on change (for
  example stem linked, stem toggle, hot cue added, track load) etc. this way we
  dont need to recompute the overview waveform canvas regularly, saving a ton of
  cpu/gpu, only the playhead needs to be rendered in the shader.

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

# MIDI/HID Mapping
- [ ] Overhaul midi mapping mode to be a  structured tree like hierarchy where the mappings are structured into logical units (action buttons, deck controls, mixer controls etc) with some questions at each logical group to figure out the style of mapping we want(do we have 2 or 4 decks, physical, virtual or momentary mode etc), instead of a list with
  questions at the front. The midi mapping guide needs to be controllable by
  just the browse encoder and its press. parent nodes should be collapsed per
  default unless we click on them, then we enter the child nodes. answering a
  questions or doing a mapping should jump to the next one (if its the last
  question/mapping in this parent node, jump to first leaf note in parent and
  unfold everything for it visually).
- [ ] The first mapping should be the encoder scroll then encoder press, so the user can interactively scroll through the steps of the mapping with just the MIDI/HID device and can decide which controls they want to remap, only those should be overwritten in the config file!.
- [ ] Before writing anything to the file we need a verification window, that
  shows all mappings that would be updated (just the control name, so the user
  can see what they are about to update). this can be "Save"d or "Cancel"d, all
  browsable via the browse encoder. For this to work we need to load existing
  mappings even for midi learn mode, so existing browse rotate + press already
  work without needing to remap.

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
- [ ] when no midi.yaml is detected while starting mesh-player automatically start in midi learn mode, so the user can map the software).
- [ ] when on the fly stem linking in the browser for selecting a linked track,
  we can utilise smart suggestions better by additionally adding specific search parameters for the stem that is about to be linked or weighting certain markers more. for example when linking drums, key is relatively irrelevant, but the energy or lufs, aggression and other metrics matters more. for vocals, key is absolutely the most important, bpm also a bit, not so much energy, for bass i think the weighting can stay as is, for other too. Its also possible that for linked stems(other than drums) very compatible key is actually a hard requirement,  and a filter for results.
