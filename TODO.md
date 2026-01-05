Here are some more features, changes and bugs, if unsure ask questions, dont
assume stuff. try to make sure that logic lies as much as possible in mesh-core
and mesh-widget and only if necessary in the ui.

# Features
- [x] Store last global bpm  via slider in the config and load it.
- [x] Looping should snap to grid (start and end)
- [x] Beatjumping while loop active should move the loop, then the playheader
  according to the beatjump length
- [x] Beatjump length should be bound to loop length.
- [x] Default loop length should be configurable in the config.
- [x] Player needs a config view as well (for now with loop length config only),
  but should be sorted in logical sections.
- [x] Add 8 loop buttons underneight the hotcue buttons (in cue not in player)
- [ ] Add support for nested playlists in the cue software. There should be a
  general collection (like a fixed special playlists), where all tracks of the collection resign, but this can be organised in
  subcollections (just ordinary playlists as well). Then there should be
  playlists, which represent playlists for live play, these should be able to
  arbitrarily nested and just represent folder structure as well. Tracks should be just linked
  to the collection from subcollection folders (softlink), so we save space.
  Users can add tracks to playlists, by moving them (drag and drop from playlist
  or collection to playlist). replace the current one-dimensional list on the
  left side with two next to each other fixed size scrollable tables next to each other underneight the track preview and edit
  section (so underneight the hot cues of the loaded track, keep this visible
  always instead of showing text, when no track is loaded). Each of these two
  tables should be the same widget (safe this in mesh widget, we might want to
  reuse that in the player) and contain 2 partsa left side hierarchy list which
  schows the nested playlists (collapsible playlists), and on the right side table headers and
  a search field, which filters the table, and the content of the table with the
  tracks of the playlists. clicking on playlists on the left side opens them on
  the right side for seeing contained tracks. Users can then drag and drop
  tracks from one to the other widget and "copy" (softlink) tracks into other
  playlists like this.

# Changes
- [x] adapt the lock free command queue pattern to the mesh-cue, but make sure
  to keep behaviour from the ui the same.
- [x] Change normal cue indicator color to gray.
- [x] The waveform indicators of hot cues use colors, but the hot cue buttons
  should be colored as well accordingly.
- [x] remove pitch slider from decks.
- [x] In both UIs in general represent button states for buttons that represent some toggleable
  action, like for example selected stem, slip, loop etc.
- [x] The 8 stem chain effect knobs should be rotary knobs, not sliders.
- [x] Rearrange beatjump, cue, play/pause, loop length selection, loop and slip
  buttons in the player. play bottom most, cue above that, beat jump left and
  right above that next to each other, above that loop related buttons and slip.
  right to cue and play there should be the 8 hot cue buttons. then above that
  all the stem section.

# Bugs
- [x] In the detailed waveform theres a small gray rectangle, what is this? if
  this is irrelevant remove, if not sure ask me.
- [x] Solo button does not work on the stems (does nothing, it should mute all
  other stems in this deck)
- [ ] The BPM calculation is not correct, for example for my Black sun empire -
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
- [ ] Round the bpm selection in the ui to ints, but make sure there is no
  rounding error in the computation then.
