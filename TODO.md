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
- [ ] since we dont have a jog wheel to nudge audio tracks, we need to ensure
  they are always perfectly in sync (on beat) during playback. for this we have the
  beatgrid, which in the player we assume is correctly in sync. so when playing
  in a track we need to make sure it snaps to the grid. i think cueing should
  not snap yet, but when pressing play we should snap to the grid. there are
  some edge cases to consider: user keeps cue pressed to preview, then decides this is
  good and presses play. expected behaviour is that the playhead jumps to the
  nearest beat and keeps playing. same for hotcues. when audio is already
  playing, pressing cue just jumps back to cue marker as usual, but pressing hot
  cue while playing should jump to the hot cues marker such that it lands
  aligned with the beat (with an offset of samples of the difference of the current playhead and the nearest beat, ie user presses hot cue 212 samples too eary, so the nearest beat is 212 samples after the current playhead, then jump playhead 212 samples before the pressed hot cues marker and analogally for when the user pressed too late.)
- [ ] in mesh-player we need to be able to scrub the playhead on the overview
  waveform just like in mesh-cue, but only when audio is not cueing/playing.
- [/] in the collection browser we need the ability for multi selection (for
  multi drag and drop) and multi deletion. Implement deletion of track from
  playlist/collection in the mesh-widget and map this in the ui via del key.
  Same for playlists. Always require confirmation via a small popup for deleting
  anything. We also need a right click menu, when clicking on tracks/playlists,
  where users can rename/delete/re-analyse.

# Changes
- [x] The waveform indicators of hot cues use colors, but the hot cue buttons
  should be colored as well accordingly.
- [x] In both UIs in general represent button states for buttons that represent some toggleable
  action, like for example selected stem, slip, loop etc.
- [x] The 8 stem chain effect knobs should be rotary knobs, not sliders.

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

# Performance
- [ ] Can we optimize how stems are stored, this is currently roughly 200-300 mb
  per multi-track file.

# Future fields
- [ ] automatic gain staging for optimal headroom and perfect sound quality
- [ ] real-time short-term lufs normalisation (that introduces no latency) per
  stem using ebur128 or lufs crate ( i want stems after processing to be
  relatively comparable loudness as input stem loudness, since rave processing can either
  be very loud or silent ).



# Later optimisations for when we plan to deploy this on fixed embedded hardware
- [ ] Currently we try to be sample rate agnostic, which is good for a wide
  potential target user hardware spectrum, but if we go embedded, we know our
  environment, we can optimize this for there (ie 48, or 96 khz sample rate fixed
  everywhere)
