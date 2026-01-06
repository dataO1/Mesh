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
    - [ ] But this also needs the corresponding logic, the deck already has
      looping capabilities, which are handled internally with state, we just
      need to wire it with ui capabilities. When pressing one of the loop button either create a new loop at the current playhead position (snap to grid, but this should happen in the deck not in the ui), which loops based on the selected beatjump width (so both
      beatjump and loop size are controlled by that). loop button should toggle
      the loop on or off and the waveform should show the loop state ( it think
      the reusable widgets already do this, but may need to be wired). This loop
      button then represents one of the 8 saved loops which we store to the
      file. When pressing a button that already has a loop, just toggle the loop
      on or off.
    - [ ] Also the stiling of the buttons is off, this should be very similar to
      the hot cue buttons, but with a loop sign on them.

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
- [ ] The bpm analysis is not working perfectly yet, all tracks are either 172
  or 178 which is wrong pretty sure, they should be in range 170 - 180 somewhere
  but more spread i think. check if this is a problem with the rounding, min and
  max bpm for detection or a tuning issue in essentia (can we somehow improve this?)
- [ ] The tracks still dont directly come up in the collection (mesh-cue) right after they
  are done with analysis. i can see them finished in the status bar and written
  as a file but not directly in the collection list in the file browser.


# Later optimisations for when we plan to deploy this on fixed embedded hardware
- [ ] Currently we try to be sample rate agnostic, which is good for a wide
  potential target user hardware spectrum, but if we go embedded, we know our
  environment, we can optimize this for there (ie 48, or 96 khz sample rate fixed
  everywhere)
