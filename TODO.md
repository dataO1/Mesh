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
- [ ] The tracks still dont directly come up in the collection (mesh-cue) right after they
  are done with analysis. i can see them finished in the status bar and written
  as a file but not directly in the collection list in the file browser.
- [ ] On resize the last state of the canvas is imprinted and does not go away.
  the actual canvas is still working normally.
- [ ] detailed zoomed waveform zoom behaviour is weird, at start and eend of the
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
  waveform colors of the stems. bass should be red/orange, vocals green, other
  cyan, drums yellow.
- [x] with the midi buttons having a hot cue pressed, then press play, then
  release hot cue stops the playback again. when the deck is playing releasing
  the hot cue (same for normal cue button) should not stop preview, but keep the
  deck playing.

# Performance
- [ ] Can we optimize how stems are stored, this is currently roughly 200-300 mb
  per multi-track file.

# Future fields
- [ ] automatic gain staging for optimal headroom and that the dj doesnt
  manually need to configure the trim knob for each track( some older tracks are
  very much not loud, while modern tracks are mastered very loud, we need to
  "normalize them" in the analysis step.).
- [ ] real-time short-term lufs normalisation (that introduces no latency) per
  stem using essentia or ebur128 or lufs crate ( i want stems after processing to be
  relatively comparable loudness as input stem loudness, since rave processing can either
  be very loud or silent ).



# Later optimisations for when we plan to deploy this on fixed embedded hardware
- [ ] Currently we try to be sample rate agnostic, which is good for a wide
  potential target user hardware spectrum, but if we go embedded, we know our
  environment, we can optimize this for there (ie 48, or 96 khz sample rate fixed
  everywhere)
