# Live Mode — Hardware Test Plan

**Date:** 2026-10-14
**Branch:** `live-performance-mode`
**Design:** [`2026-10-13-live-mode-design.md`](2026-10-13-live-mode-design.md)

Write each result next to its ID (pass / fail / note). Musical results go into the design doc's change log.

## Setup (PC)

```sh
cd ~/Projects/Mesh
direnv exec . cargo run --release -p mesh-player -- --live ~/Music/mesh-live-dev/styles/starter-breaks
```

- The F1s need the udev rule `etc/udev/99-mesh-hid.rules` in the NixOS config.
- Without F1s, the keyboard works (`crates/mesh-player/src/live/keys.rs`).
- To rebuild the starter style: `scripts/live-starter-style.sh <dir> --force`.

**Offline tools:**

| Tool | Use |
|---|---|
| `live-render --style <dir> --out x.wav --bars 16` | Render bars to a WAV file |
| `live-patterns --style <dir>` | Print generated patterns as ASCII step grids |
| `live-bench --style <dir>` | Measure CPU cost |
| `style-calibrate <dir>` | Set loudness trims |
| `clap-scan` | List CLAP instruments and their capabilities |

## A. Engine and sound (P1, PC)

| ID | Check | Pass when |
|---|---|---|
| A1 | Style loads, audio starts, no xruns for 10 minutes | No dropouts, `starved` stays 0 |
| A2 | Each lane alone (mute the others): drums, bass, lead, extra | Each sounds clean, no clicks at slice edges or note ends |
| A3 | Kit browse on drums (hold Size + encoder, release) | Kick/snare pair and break swap on the bar, the pools stay, no gap, no click |
| A4 | Bass option swap sampler ↔ Surge XT | Swap at the bar, Surge plays notes, glide works on tied notes |
| A5 | Relative levels of the 4 lanes after `style-calibrate` | No lane buried or dominant at default controls |
| A6 | Glue: kick ducks the bass, sub stays mono, drum bus glue, master limiter | Low end is clean, no pumping artefacts, master never clips |
| A7 | Delay/reverb sends on lead and extra | Tempo-synced delay, reverb tail fades cleanly |
| A8 | Sound sign-off per role (design gate P1) | You would play these sounds |

## B. Pattern core and harmony (P2, PC)

| ID | Check | Pass when |
|---|---|---|
| B1 | Timing tightness of all lanes against each other | No flams, no drift over 10 minutes |
| B2 | Seed change (hold Play B + encoder) lands on the next bar; a Play B tap returns to the previous seed | Never mid-bar, the tap swaps back and forth |
| B3 | Lane toggle with Quant on vs off | On: lands on the bar. Off: immediate, short fade |
| B4 | Key intent: Shift + left fader 1 up and down | Target key shows in the header and on the 7-segment, nothing changes until commit |
| B5 | Commit key (Shift + encoder push) | Change lands on the next phrase start, all pitched lanes move together, rhythm unchanged |
| B6 | Progression change (Shift + left encoder) | Applies at the phrase, bass roots follow |
| B7 | Tempo: Shift + left fader 2 | Ramp over 4 bars, no glitch, breaks follow |
| B8 | Half-time (Reverse) | Drums switch to half-time feel, grid unchanged |
| B9 | Momentary moves: stutter, fill, double, void | Instant response, clean release back to the pattern |

## C. Generators (P3, PC: musical ear test)

| ID | Question | Where to change it |
|---|---|---|
| C1 | Does procedural output sound musical or random at default settings? | Overall verdict for D42. If random, try the idiom family |
| C2 | Style knob: blend between archetypes, or hard switch? | `generator.params.blend` (0/1) |
| C3 | Archetype order half-time → two-step → rolling → jungle → hardcore | `archetypes:` list |
| C4 | Density sweep: voices come in in order (hats/perc, shaker, ride, top loop) and fall back to the backbone cleanly | Drums fader A, `enter_*` / `overlay_*` in the style |
| C5 | Hats with strict backbone lock (A2) leave gaps on kick/snare steps. OK? | `rule_a2` 0 / 0.5 / 1 |
| C6 | Break mutation: original order mostly kept; ghosting off-backbone kicks/snares helps or hurts? | `rule_a4`, `chop` |
| C7 | Phrase shape: bars 1–3 repeat, bar 4 varies. Is the bar-4 ruff too predictable? | `variation`, `rule_a3` |
| C8 | Fill length (half a bar) | Fill pad |
| C9 | Bass vs kick: avoid, ignore or double? | `kick_mode` |
| C10 | Bass note choice root-heavy; slides and accents | `complexity`, `slide`, `accent` |
| C11 | Lead motif sparseness; arp and stab modes; arp thinning under busy drums | `mode`, `density`, `rate` |
| C12 | Key changes keep everything in key | B5 plus listening |
| C13 | Could you play 30 minutes and keep it interesting? | Overall P3 gate |

`live-patterns` shows the patterns behind any C finding.

## D. Control surface (P4, PC with both F1s)

| ID | Check | Pass when |
|---|---|---|
| D1 | Left/right assignment (first Shift press, or `live.f1_left_serial`) | Both units distinguished. Check that the two F1s report different USB serials |
| D2 | Pads: drum mutes (kick snare / hats ride / shaker perc / top break), Shift + 7/8 form; pitched oct− oct+ / half dbl / form◀ form▶; Sync = moves | Feels natural, or note the alternative |
| D3 | Tap vs hold on play A (300 ms window) | No accidental toggles while browsing |
| D4 | Soft takeover on knobs and faders after recall or kit swap | No value jumps |
| D5 | LEDs: lane colours, muted voices dim, queued mutes and forms blink, octave / rate state, held move pads, Play B lit while held | Readable at a glance, off-lane level visible |
| D6 | 7-segment: key (`8A`, `8b`, `AA` = 10A …) and tempo (last 2 digits) | Legible enough |
| D7 | Capture (lane, Shift = all) | File appears in `captures/` with seed, form, octave and mutes (no F1 recall) |
| D8 | No menu diving for a 30-minute set | P4 gate |

## E. Screen (P5, PC)

| ID | Check | Pass when |
|---|---|---|
| E1 | Header: key → target, BPM + ramp, half-time, progression, phrase cells | Correct and stable |
| E2 | Code readout per lane: values update, changed values outline, firing parts pulse | Text never shifts, highlights in sync with sound |
| E3 | Scopes per lane in the lane colour | Smooth, matches the sound |
| E4 | Pad mirror per lane and the form with its playing segment in the readout | Matches the F1 pads and what plays |
| E5 | Error screen: start with a broken style name | Clear issues listed |
| E6 | Looks good enough to project | P5 gate |

## F. Data lifecycle (P6, PC)

| ID | Check | Pass when |
|---|---|---|
| F1 | Copy the starter style into `<collection>/styles/`, export a playlist to USB from mesh-cue | `styles/` arrives on the stick, re-export skips unchanged files |
| F2 | Recording (`Ctrl+r` or `Message::ToggleRecording`) | WAV in `mesh-recordings` on USB or local |
| F3 | Captures written to USB during the set | Files in `<usb>/mesh-collection/captures/<style>/` |

## G. Orange Pi 5 (embedded target)

| ID | Check | Pass when |
|---|---|---|
| G1 | aarch64 build of the image with the live code (FPCR denormal asm in `rt.rs`) | Builds and boots |
| G2 | `live-bench --style <usb style>` on the board, Hermite and Sinc8, 0/1/2 CLAP lanes | Worst block well under 5.33 ms. Sets voice budgets and D32 |
| G3 | `clap-scan` on the board | Which synths load. Per-note modulation and TUNING support |
| G4 | Live mode from the USB stick (`mode: live` + `live.style` in `player-config.yaml`) | Style loads from USB, plays |
| G5 | Core pinning: audio on A76 5–7, renderer on A55 1–3 (`ps -eLo psr,comm`) | Threads on the intended cores |
| G6 | Xruns over 30 minutes with current boot tuning (A55-isolated) | If xruns: decide mode-aware IRQ/idle services (design change log 2026-10-14) |
| G7 | GPU: frame time of the live screen at 2880×864 on Mali GL | No stutter, visuals at 30 Hz |
| G8 | Both F1s on the board, full set run | Same as D1–D8 |
| G9 | RAM after loading a full style | Well within 8 GB, sets the style size cap |
