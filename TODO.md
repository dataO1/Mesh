Here are some more features, changes and bugs (the unticked tasks, the ticked
one you can assume as done already), if unsure ask questions, dont
assume stuff. try to make sure that logic lies as much as possible in mesh-core
and mesh-widget and only if necessary in the ui.

# Features
- [x] Store last global bpm  via slider in the config and load it.
- [x] Default loop length should be configurable in the config.
- [x] Player needs a config view as well (for now with loop length config only),
  but should be sorted in logical sections.
- [x] Add 8 loop buttons underneight the hotcue buttons (in cue not in player)
    - [/] But this also needs the corresponding logic, the deck already has
      looping capabilities, which are handled internally with state, we just
      need to wire it with ui capabilities. When pressing one of the loop button either create a new loop at the current playhead position (snap to grid, but this should happen in the deck not in the ui), which loops based on the selected beatjump width (so both
      beatjump and loop size are controlled by that). loop button should toggle
      the loop on or off and the waveform should show the loop state ( it think
      the reusable widgets already do this, but may need to be wired). This loop
      button then represents one of the 8 saved loops which we store to the
      file. When pressing a button that already has a loop, just toggle the loop
      on or off.
        * [ ] buttons are there and styled correctly, but dont do anything (they
          are greyed out as well)
    - [x] Also the stiling of the buttons is off, this should be very similar to
      the hot cue buttons, but with a loop sign on them.
- [x] since we dont have a jog wheel to nudge audio tracks, we need to ensure
  they are always perfectly in sync (on beat) during playback. for this we have the
  beatgrid, which in the player we assume is correctly in sync. so when playing
  in a track we need to make sure it snaps to the grid. i think cueing should
  not snap yet, but when pressing play we should snap to the grid. there are
  some edge cases to consider: user keeps cue pressed to preview, then decides this is
  good and presses play. expected behaviour is that the playhead jumps to the
  nearest beat and keeps playing. same for hotcues. when audio is already
  playing, pressing cue just jumps back to cue marker as usual, but pressing hot
  cue while playing should jump to the hot cues marker such that it lands
  aligned with the beat (with an offset of samples of the difference of the current playhead and the nearest beat, ie user presses hot cue 212 samples too eary, so the nearest beat is 212 samples after the current playhead, then jump playhead 212 samples before the pressed hot cues marker and analogally for when the user pressed too late.). this should happen automatically and internally in the deck, abstracted away for ui and should be controllable via a config flag and from the ui.
- [x] in mesh-player we need to be able to scrub the playhead on the overview
  waveform just like in mesh-cue, but only when audio is not cueing/playing.
- [/] in the collection browser we need the ability for multi selection (for
  multi drag and drop) and multi deletion. Implement deletion of track from
  playlist/collection in the mesh-widget and map this in the ui via del key.
  Same for playlists. Always require confirmation via a small popup for deleting
  anything. We also need a right click menu, when clicking on tracks/playlists,
  where users can rename/delete/re-analyse.
- [x] Stem Slicing (if you have any questions or are unsure of what i mean, please ask before, so we can be
  sure you understand my intention)
  - [x] Description: I want another module in the audio engine, which is used
    for mangling the arrangement of a stem during playback, think of it as real
    time remixing the stems. The user should enter slicer mode, which for a stem
    of a deck (independent of other stems or decks) slices a fixed size of the
    buffer (configurable from the global config, default 4 bars, snapped to grid) into equal 8 parts. these 8 parts can then be triggered form the action buttons. The slicer remembers the order in which the action buttons got pressed, which becomes the new "arrangement". Now the playback of the deck in the background continues normally and every time it surpasses the bounds of the fixed slicer buffer size  the slicer fills it with new buffer infos(kind of like a loop with its range, that keeps moving forward in fixed steps snapped to grid). the buffer plays with the new order ("arrangemnet") with the information from the deck playback. this output is then routed further to the audioengine as the current playback.
  - [x] Architecture: Should be its own module, where all audio buffer of a deck runs
    through after the buffer is loaded (in the deck) but before the effects chain. seperation
    of concern is important this is independent for each deck, this module should just get a slice of the buffer,
    which can be updated, based on this buffer the slicer module computes a new
    buffer, which is given to the effects -> jack output.
  - [x] UI: above the hotcues and cue button and underneight the beatjump, loop,
    slip etc, there should be another row with a shift button at the left that
    is as wide as the cue button then next to it (with some space between)
    buttons for the action button modes. the first mode which is already
    implemented is hot cues, the second mode will be slicer. whatever is
    selected defines the hot cues behaviour and visuals. there can only ever be
    one mode selected. hot cuee behaviour and visuals is good as is. in slicer
    mode each action button fills the slice buffer sequentially, depending on
    the slice buttons indexes content. button 3 "contains" the third piece
    divided of equal size of the original buffer. pressing shift + slicer should
    stop slicer mode and the output just flows through the slicer module.
- [x] new feature: midi controller compatability
  - [x] we need a separate midi.yaml which is automatically loaded on startup
    like the other config, which defines a mapping from midi channels and
    message to all potential interaction possibilities. a user could potentially
    have many mapping files for different devices, that follow a specific format
    (research available rust crates, that implement parts of this), but only a
    single active one should be loaded. so lets say i have a
    midi device, i want a specific knob to handle the value of a certain
    interaction point of the application. so we need some sort of abstraction
    around the app, which lets it define mappable interactions. then we can map
    these to midi capabilities (from multiple devices potentially, so multiple
    channels?). this should be able to normalize between value
    ranges (midi is usually from 0-127, the application abstraction layer needs
    to take this and map this to the required value range from the application).
    this needs to be fully modular and the app should be easily adjustable to
    add new interaction points.
  - [x] initially each knob/slider from the player ui should be mappable. also
    the scrolling and loading of tracks from the browser.
  - [x] this should be its own module, potentially with a trait, that defines
    interaction points or something? think of a good abstraction pattern
  - [ ] for backwards compatability with old devices (like sb2) we should
    support beat nudging via the jog wheel. this needs to work with the current
    snapping system, that when a user nudged a beat a certain amount of samples,
    this needs to be remembered that after beat jumping/hotcue presses and other
    seek operations this offset is kept in mind, so the dj doest need to nudge
    again.
- [x] the zoomed and overview waveform should adjust alpha value for currently
  not active stems. They should be visible to be inactive. make them different
  gray tones (drums darker, bass dark, vocals middle, other light)
- [x] the midi exploration needs to detect various kinds of midi
  messages/hardware types, like standart knobs, sliders/faders, encoders etc
  with their various midi message types and while mapping defines this smartly
  in the config and be able to automatically defer the range and format. for
  example currently the stem for other is expecting a button , but i only have
  a poti knob, the midi mapper needs to understand this during midi
  exploration and corrrectly map this. here are some message types for my
- [x] hot swapping of stems in a loaded track with another tracks stem ( we
  should keep both options in memory, so the user can hot swap between them )
  - [x] several possibilities
    - [x] prepared in mesh-cue:  user can per track per stem define a linked
      other tracks stem by pressing one of four stem link buttons, then select a
      another track in the browser, which links the selected tracks stem to the
      track the dj is currently cueing (this does not copy the data over, but
      only references the track name in the collection, the dj software should
      load this smartly if defined). so for example when the user is cueing
      track A and presses the vocal stem link button (underneight the hot cue
      and loop buttons) the file browser is focused with highlighting that
      indicates the stem color and that a stem of another track is being linked.
      on selecting a track and clicking enter the vocal stem of selected track B
      is linked into the wav (via headers) into track As wav form. the player
      then knows how to handle this.
    - [ ] unprepared/on-the-fly: this is the more important case, the user
      should be able to press a button combination (like shift + stem button),
      which redirects to the browser (just like in the other scenario) with
      highlighting and indicating that another track for stem linking is
      expected. with encoder press this is allowed then. this should populate
      the players linked stem buffer with the stem of the selected track
      (matching the grid, this is very important!). the new stem is only
      prepared, not running yet, on pressing shift + stem button on a loaded
      linked stem toggle between original stem and buffered stem. this should
      happen all internally in the deck and be abstracted away from the ui and the ui should only send high level commands. waveform should also visually indicate linked stems and on toggle show the correct buffer info (i think what is being rendered is linked to what is played, so this should require no/minimal changes to the waveform canvas itself).
    - [x] This needs to happen in the deck and before the slicer so switching to linked stem works woth slicer out of the box
    - [x] How to handle synchronisation between original and linked stems with potentially different bpm, beatgrid and structure?
        - [x] Maybe we need to actually have a pre-stretched stem in the wav file? I would like to avoid that. Maybe we need a prestretch phase reusing the timestretching architecture we already have only for linked stems in the deck. I prefer that.
        - [x] I think we actually need to restrict linked stems to prepared mode, so we can define a link point which marks a common first beat, for example marking the drop. Or we have structural markers for tracks anyways, the we can use on the fly stem linking. We could use the hot cue marker for relative positioning, but how is the user interaction then and how does the system k or which hit cues match? I think I like the drop marker
        - [x] How to visualise this? I think we can split the detailed waveform horizontally in half, the upper half represent the actively playing stems, the bottom half the linked stems (if there are any, otherwise keep full waveform). Same for overview waveform, so the dj has an overview of when stems have meaningful information (like a vocal stem which does not permanently have info, also the alignment becomes visual then).
    - [x] if you still have questions let me know and lets design this together.
- [x] we need usb stick support, so in mesh cue the user should be able to
  export playlists, and config files to external devices (like usb stick) of the
  right format. everything that is necessary for
  mesh-player to work and the dj to play sets on it with their saved config
  file. what should not be transfered: midi mappings.
  - [x] the file browser needs to be adjusted. for mesh-cue show the locally existing
    collection and playlists as before. Add a small icon in the footer for
    opening a export manager popup (similar to the import window), where in a
    header the user can select detected external storage devices with supported
    file systems, that can be mounted. selecting a device should mount it if not
    already mounted, detect if there is a collection/playlists/config setup
    existing. under the header, there should be two columns, left side for the
    local playlists, right side the selected external devices playlists. The
    user can then in the footer press a "export" button to sync local selected playlists (from the left column) to the usb stick.
  - [x] the sticks exported format should also be like local: all track files in
    the colleciton and playlists with symlinks to the collection. the export
    process should be as efficient as possible, so check existing files in the
    collection (via hash to compare if they got updated) and only sync new
    files. this should use multi-threading efficiently as well.
  - [x] in mesh-player the file browser needs to automatically show detected
    external usb devices with exported playlists. so on the otp level there
    should be devices (with their name), then if click on it, show the playlists
    inside (similar to already existing playlists behavirou for local).
  - [x] all the usb detection, mounting, export import logic for playlists etc,
    should be written ONCE, be its own logic unit in mesh-core and should be
    initialized once each for mesh-cue and mesh-player each. the uis should
    communicate with it via messages and subscriptions efficiently.
- [ ] mesh-player should keep dj history for each session, per dj and persist it
  on the db while playing
  - [ ] for now this is primarily used to update the graph based relations for
    track exploration features using the vector/graph db, but later this should
    be able to be used for full set reconsttruction.

# Changes
- [x] The waveform indicators of hot cues use colors, but the hot cue buttons
  should be colored as well accordingly.
- [x] In both UIs in general represent button states for buttons that represent some toggleable
  action, like for example selected stem, slip, loop etc.
- [x] The 8 stem chain effect knobs should be rotary knobs, not sliders.
- [x] Change minimum loop length to 1 beat and maximum to 64 bars.
- [x] remove slider next to mute and solo button for stems.
- [x] Lets work on the player ui, the overall layout is mostly fine, but i dont like
  the deck controls layout, we need to clean this up properly. Also the overview
  waveform of each track should be underneight the zoomed waveform, so also in
  2x2 grid. Then next to the respective waveform section of the deck i want the
  controls. They should be just as high as the two waveforms for the deck. Instead of the header for the deck in the controls i want the decks number (just the number) and the loaded track ias a header row above the zoomed waveform(it must be part of the canvas i think, since the canvas needs to be a single unit due to the iced bug). Then next to the respective canvas of the deck i want the controls, and they should be just as high as the waveforms for the deck (so the controls of 1 and 3 should be left of waveform 1 and 3 and 2 and 4 right to 2 and 4). In these controls i want a single full span row with the stem controls. on the left side of everything a header with 4 buttons for the stem selection. on the right side two rows 1 with the chain and 1 for the 8 potentiometer knobs, full spanning width. Underneight the stem controls i want two columns (20% and 80% width). The left column contains a row for loop and slip, then a row for loop/jump length selection (as it is right now), then a row for beatjump buttons, then a row for big cue buttons, then a row for big play/pause button. on the right column i want the 8 performance pads fully filling height and widght equally spaced.
- [x] Slicer preset mode should be the default mode and instead on shift +
  action button we want to assign the action buttons slice buffer to the current
  timed queue slot and preview the slice (one-shot style like in the current "replace" mode of the slicer). also remove the fifo implementation and config settings, this is not working well in live play, but keep the affected stem selection and buffer size selection in the config.
- [/] change the presets to some breaktbeat relevant patterns (assume a dnb
  breakbeat/2step pattern incoming which is chopped into 16th, then rearrange
  them to interestingly musically relevant new patterns instead of the current
  algorithmic patterns). They should be sorted from "empty sounding" (pattern 1)
  to most busy, repetetive sounding(pattern 8).
- [x] ok now that we have midi support, we want to distinguish between performance
  mode and mapping mode. the default mode with no flags should launch the player
  in performance mode, where the layout is much simpler, just the canvas with
  the waveforms on top and the file brwoser in the bottom (around 60% height and
  40% height for the browser). make sure to not have code duplication, all the
  logic should be not in the ui itself, but factored out if necessary (like
  engine behaviour and interaction handlers etc) and the layout should reuse the
  existing components.
- [x] mesh-cue needs to be able to toggle mute state of stems and also load
  stem-links and be able to switch between stems (just like mesh-player, this
  should reuse all the decks capabilities and not introduce duplicated code). we
  already have 4 stem link button, when a stem link button is set with a stem
  link pressing this should toggle between the stems.
- [x] add auto gained db difference to the decks header (for example +2dB or
  -3dB)
- [x] for the auto gain, should we have a clipper on the master for safe
  playback?? no just use a sane lufs target so tracks dont get boosted too much!
- [ ] Slicer rework/update (all of the logic needs to be in the slicer
  module/file and reuse or adapt existing code patterns to avoid duplicate code
  and multiple code paths):
  - [ ] We need more features and possibilities:
    - [x] Velocity per step (for ghost snares)
    - [x] optionally multiple layered slices on a slice pad/buffer (up to 2 for
      now, but add a const value, which can be changed later)
    - [x] currently we have a single preset/buffer for the whole track and
      control which stems are sliced via config. we want a potential per stem
      preset buffer (so each stem has its own queue/arrangement, but the user
      just needs to press a single preset button in any slice, that way the user
      can build coherently working presets for bass/drums etc). If a preset does
      not set a stems behaviour bypass slicer for this stem (ie. if the preset is set
      for drums only, only slice the drums, if the preset defines different
      queue for drums and bass, slice them each with their own arrangement and
      bypass the rest).
    - [x] slices can be "muted" (set to 0), which enables us to have slower
      feeling beats like 1,0,2,0,3,0,4,0,5,0,6,0,7,0,8,0 . slices that have a
      muted slice afterwards should have a release fade off to avoid
      clickiness.
  - [x] Then with the new slicing features we need a slice preset editor in the
    mesh-cue software, where users can prepare slice presets based on a loaded
    track.
    - [x] Slices are still stored in the config, not per track, the track is
      just there for reference of the beat, so the user can interactively create
      presets.
    - [x] We need a slice edit widget (this should be its own widget/file in the
      shared widgets and hide logic from the outside and the ui should just
      communicate with it). for the overall layout: 2 colums, the left colum has
      4 rows/buttons for the stems, then in the second column a 16 by 16 grid
      (0,0 is bottom left) which should look similar to a midi editor in a daw. the x axis represents
      the queue slot to be filled, the y axis represents the possible slices to
      set. clicking one of the buttons should toggle the slot on or off, for now no velocity, we will add this later. for example
      pressing button 4x1 (x axis 3 y axis 1) and button 4x8 assigns two slices (1 and 8) of the original buffer to the queue slot 4. so on position 4 there will be two slices playing at the same time. toggled on buttons should be black, others white if default queue position (so x=y) or gray if the column is muted. the buttons of the grid should be flush next to each other, not padding and the buttons should not be rounded, but rectangular. also the buttons should be wider than high and fixed size. there should be a header row with a button for each column, which on press toggles the whole column muted or not.
    - [x] the mesh-cue waveform should also show the slice mode like in the
      mesh-player, make sure to reuse existing code for this/factor this out
      for common use ,make sure to not duplicate code and the logic should
      resign in the widget, not in the uis, the ui just sends commands with
      necessary information.
    - [x] the stem linking buttons should be moved to a column with 4 rows for
      each button right to the waveforms canvas, fitting the height.
    - [x] now the slice widget lives underneight the queue and loop buttons.

- [ ] a single morph knob for slicer per deck. this should scroll through a
      presets preset banks. preset banks have up to 8 presets



# Bugs
- [x] Solo button does not work on the stems (does nothing, it should mute all
  other stems in this deck)
- [x] the playback speed in mesh-cue feels to fast, do we have timestretching
  active there? this should be disabled for mesh-cue.
- [x] The BPM calculation is not correct, for example for my Black sun empire -
  feed the machine track from the collection (which is originally 172 bpm) if i stretch this to 149
  in the ui it actually is around 163 bpm. this has to do something with the
  slider ui, or actual calculation of the bpm/timestretch. make sure this is
  robust and has no rounding errors, such that there is no drift between
  different tracks. if i have another track which is originally 174 bpm
  (pendulum witchcraft from the collection), they are nicely locked in the
  beginning, but after a while i think they drift apart. this hints that
  probably the logic is wrong or there is some rounding errors/sample mismatches
  in the stretching. it could also be the case that witchcraft has some empty
  seconds in the beginnging before the first beat, which affects the total
  numver of samples, which could affect the computation. Pendulum Witchcraft
  alone on 149 on the ui is actually ~162 bpm (measured by tapping in).
- [x] Round the bpm selection in the ui to ints, but make sure there is no
  rounding error in the computation then.
- [x] The bpm analysis is not working perfectly yet, all tracks are either 172
  or 178 which is wrong pretty sure, they should be in range 170 - 180 somewhere
  but more spread i think. check if this is a problem with the rounding, min and
  max bpm for detection or a tuning issue in essentia (can we somehow improve this?). We should try to also just do bpm detection on the drums track, since we have the stems.
- [x] detailed zoomed waveform zoom behaviour is weird, at start and eend of the
  track, when there is not enough buffer information, we need to pad
  beginning/end of the buffer with zeroes (only in the waveform internally for
  visual computation)
- [x] while looping, the loop is either not perfectly grid aligned, or theres
  also some fractions of samples lost, this should stay perfectly in sync as
  well.
- [x] while looping beatjumping should beatjump as is right now, but the loop area
  needs to snap to grid.
- [x] the slicer visual resolution depends on the previous zoom level from the
  "normal" mode. so when im zoomed out i nthe normal mode the visuals in the
  slicer are too low-res, when zoomed in they are good. solution: set the zoom
  level per mode (fixed waveform mode should have its own fixed resolution based
  on the config selected slicer buffer length. if 1 bar, the resolution should
  be high.)
- [ ] the colors of the stem status in the waveform do not match with the
  waveform colors of the stems and also the waveform stem colors are different
  for zoomed and overview waveform. make this uniform and conigurable via a third style config, that is read like the other on startup (called theme.yaml), all color styling related config should lie in there and be read from the static config service. default bass should be red, vocals green, other cyan, drums yellow/orange.
- [x] with the midi buttons having a hot cue pressed, then press play, then
  release hot cue stops the playback again. when the deck is playing releasing
  the hot cue (same for normal cue button) should not stop preview, but keep the
  deck playing.
- [ ] it seems the deck virtual deck toggle buttons need similar logic like the
  action pad modes. on the ddj-sb2, the deck toggle buttons make the deck
  specific buttons have their own channel (action buttons, mode switches)
- [x] there is still problems with the interdeck syncing of two beats. when one
  track is playing and i add another by pressing hot cue, play, then hot cue
  release, they are not synced. either
  the grids are not perfectly aligned (you should assume they are) or there is
  some bug with the aligning when pressing play,hot cue buttons etc or we simply
  dont use the syncing logic in this case. make sure
  this also works when triggered through midi mappings. also make sure we
  properly use fractional sample correction correctly so the playback does not
  drift. also validate the code quality and duplication of this. can we improve
  this, by making this better factored out, such that all the correction,
  playback logic etc happens at a central point like the deck/engine.
- [x] clicking backdrop or closing the export window stops the export process,
  this should run in the background. the footer shows the progress.
- [x] auto select the first external disk in the export window.
- [x] mesh-player again doesnt show any tracks in the playlists. check the usb
  filesystem and verify the export is correct.
- [x] deleting playlists on the usb stick (so the symlinks), then reexporting
  them, copies the tracks as well (even though they are still present). so for
  playlists, before copying the files, check if they are already present.
  tracks can also be present in several playlists, so this should never
  duplicate tracks, but first check if the track is present in /tracks, if not
  copy, then symlink it (for ext4).
- [x] stem linking visuals:
  - [x] for overview wvaeform: always show on top half the currently running
    stems together, on bottom half any non-running stems. this needs to be
    aligned against the drop markers, not just rendered blindly. so when i have
    a track where the vocal stem is stem linked to another vocal stem of another
    track, show this vocal stem initially in the bottom half, as soon as i
    switch, switch this stem to top and move the original stem to bottom half.
- [ ] when importing, tracks still dont directly come up in the collection (mesh-cue) right after they
  are done with analysis. i can see them finished in the status bar and written
  as a file but not directly in the collection list in the file browser.
- [ ] On resize the last state of the canvas is imprinted and does not go away.
  the actual canvas is still working normally.
- [ ] when deleting a file in the file browser, first mark the next one (or
  previous one if there is no next one) for selection, otherwise it scrolls to
  the very top.

# Performance
- [ ] Can we optimize how stems are stored, this is currently roughly 200-300 mb
  per multi-track file.
- [x] we can probably compute the high resolution peaks during cueing and store
  them in the wav file instead of computing them on the fly (~30ms per stem).
  this only works for stems of the original file not for linked stems, since
  they need to be prestretched, then the peaks get computed. NO WE CANT DUE TO
  GAIN STAGING.

# Future fields
- [x] automatic gain staging for optimal headroom and that the dj doesnt
  manually need to configure the trim knob for each track( some older tracks are
  very much not loud, while modern tracks are mastered very loud, we need to
  "normalize them" in the analysis step.). So while analysing the track measure
  the integrated lufs loudness of the whole track summed (all 4 stems together)
  using essentia library (which we already have as a dependency, research the
  api and how to do lufs analysis). then store the integrated lufs value in the
  tracks tags. then later in the player the player should automatically
  compensate the gain of the track to a specific target (which is configurable
  in the configuration ui, default -6 LUFS). so tracks that have a higher lufs
  value (like -3 LUFS) should get turned down 3 db, tracks which are less loud
  should be turned up. importantly the waveform needs to reflect this, so the
  computed peaks height need to be scaled for diplay.
- [ ] real-time short-term lufs normalisation (that introduces no latency) per
  stem using essentia or ebur128 or lufs crate ( i want stems after processing to be
  relatively comparable loudness as input stem loudness, since rave processing can either
  be very loud or silent ).



# Later optimisations for when we plan to deploy this on fixed embedded hardware
- [ ] Currently we try to be sample rate agnostic, which is good for a wide
  potential target user hardware spectrum, but if we go embedded, we know our
  environment, we can optimize this for there (ie 48, or 96 khz sample rate fixed
  everywhere)
- [ ] use cpal instead of jack for full cross-compatability
