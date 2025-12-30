# Tech Stack
Rust based audio engine and UI for both the DJ Player and Cue Software. For UI use
rust iced.

# Architecture
Multi-track synced playback engine, with look ahead audio manipulation and
real-time effects.
- [ ] Preparation (Cue Software)
    - [ ] Stem separation using UVR5 -> multi-track wav
    - [ ] Synced tracks/stems stored in a multi-channel WAV (RF64/ BW64) file,
  which supports scrubbing, cue points (with cue and adtl chunks) and metadata
    - [ ] Cue Software with track analysis (key, bpm, beat grid), preview Player (reads tags), which can beat jump and edit cue points (in memory, then on safe to the file).
- [ ] DJ Player
    - [ ] up to 4 decks of time synced (bpm, on beat grid) parallel processed lanes
        - [ ] keeps track of all lane effect chain delays (number of sample buffer
      size) and syncs them (uses the biggest buffer size as the target delay for
      all lanes dynamically). Latency Compensation using ring buffers or delay
      line!
    - [ ] Buffers the loaded tracks/stems for each deck in memory for real-time
      and look ahead audio manipulation
    - [ ] Global Effects:
        - [ ]
    - [ ] Per Deck effects:
        - [ ] 3 Band DJ EQ, Gain, HP/LP filter
        - [ ] Beat jumping (jumps each lane of the deck)
        - [ ] Jump to hot cues (jumps each lane of the deck)
        - [ ] Adjustable Beat grid loops (loops each lane of the deck)
        - [ ] Quick loops (like Denon, future feature, not in initial version)
        - [ ] Beat Mangler (like Denon, future feature, not in initial version)
    - [ ] Per lane processing
        - [ ] Per stem modular effect chain (known buffer size for stem syncronization) with support for:
            - [ ] external puredata effects via libpd-rs (with dynamic latency
              detection using signals from the patch and subscriptions in the
              rust effect definition) (dynamically link/compile puredata required externals like nn~ which required GPU libtorch support)
                - [ ] mapped controls/parameters via effect abstraction (using
                  pd's PdGlobal for messaging)
            - [ ] loading CLAP plugins support (research which library is the
              best for our use-case)
                - [ ] mapped controls/parameters via effect abstraction
            - [ ] custom high level composition of various effects into a chain
              (with parameter mappings), which then can be used on lanes
              chain, which can be used on a lane.
        - [ ] Latency Compensated Summed stems are timestretched and pitched via [signalsmith-stretch](https://docs.rs/signalsmith-stretch/0.1.3/signalsmith_stretch/struct.Stretch.html) to the global target
