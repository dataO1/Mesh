# Traktor Kontrol X1 MK2 — HID Integration Plan

## Device Identity
- **Vendor**: Native Instruments (VID `0x17CC`)
- **Product**: Kontrol X1 MK2 (PID `0x1220`)
- **Protocol**: Class-compliant HID (no proprietary driver needed)
- **MK1 (PID `0x2305`)**: Avoid — uses proprietary protocol, not class-compliant

## Hardware Overview

| Component | Details |
|---|---|
| Buttons | 39 (RGB-capable LEDs) |
| Encoders | 4 push encoders (2 per deck side) |
| Knobs | 4 potentiometers (analog, 12-bit / 4096 steps) |
| Touch Strip | Capacitive, 11-bit resolution (0x0000–0x07FF), multi-touch (2 points) |
| Touch Strip LEDs | 21 RGB segments |
| 7-Segment Display | 2 groups (one per deck side), likely 3 digits each (24 bytes per group) |
| Phase Meters | 2 (one per deck side), multi-segment |
| Backlight | Controllable brightness |

## Physical Layout

Mirrored dual-deck design (left = Deck A, right = Deck B):

```
 [Knob1] [Knob2]          [Knob3] [Knob4]
 [Enc1]  [Enc2]           [Enc3]  [Enc4]
 [FX buttons ×4]          [FX buttons ×4]
 [7-seg display]          [7-seg display]
 [Transport buttons]      [Transport buttons]
 [Pads ×4]                [Pads ×4]
           [Touch Strip (shared)]
           [Browse Encoder]
```

Maps naturally to our 2 physical decks -> 4 virtual decks (via layer toggle).

## HID Protocol

### Input Reports (device -> host)
- **Report ID**: `0x01`
- **Size**: ~24 bytes
- **Content**: Button bitmask + encoder positions (4-bit, 16 positions/rotation) + analog knob values (12-bit packed) + touch strip position + touch timestamps
- **Pattern**: Same delta-detection as F1 driver (compare against previous report)

### Output Reports — LEDs (host -> device)
- **Report ID**: `0x80`
- **Size**: ~52 bytes
- **Content**: Button LED brightness (0x00–0x7F), RGB pad colors (3 bytes each: R, G, B)
- **Brightness range**: Same as F1 (0x00 = off, 0x7F = full)

### Output Reports — Display & Touch Strip (host -> device)
- **Report ID**: `0x81`
- **Size**: ~75 bytes
- **Content**:
  - Bytes `0x01`–`0x18` (24 bytes): Deck 1 7-segment display
  - Bytes `0x19`–`0x30` (24 bytes): Deck 2 7-segment display
  - Bytes `0x31`+: Touch strip 21 RGB LED segments

### Critical: Post-Write Handshake
After every output write to endpoint `0x01`, the host **must read 1 byte from endpoint `0x81`**. Without this acknowledgement read, the device stops accepting further output reports (LEDs/display freeze).

This is different from the F1 (fire-and-forget writes). Implementation options:
1. Add `post_write_handshake` method to `HidDeviceDriver` trait (preferred)
2. Extend `HidIoThread` to support a secondary read endpoint
3. Handle in driver-specific I/O thread variant

## Mesh Control Mapping

| X1 Control | Mesh Target | Notes |
|---|---|---|
| 4 knobs (2/deck) | FX macros | 12-bit = 32x finer than MIDI CC |
| 4 push encoders (2/deck) | Loop size, browse | Encoder rotation + press |
| 4 FX buttons (per side) | FX enable / pad mode select | RGB LED feedback |
| Transport (play/cue/sync) | Direct deck transport | Per-button LED |
| Flux buttons | Slip mode toggle | |
| 4 pads (per deck) | Hot cues / slicer (4 slots) | Full RGB feedback |
| Browse encoder | Browser scroll + select (press) | |
| Touch strip (input) | Seek/scrub or FX parameter sweep | |
| Touch strip (LEDs) | Playhead position / loop range | 21 segments = ~5% resolution |
| Phase meters | Beat phase indicator | Visual metronome |
| Backlight | Active deck / layer color | |

### Pad Limitation
4 pads per deck (vs F1's 16 grid buttons). For hot cues this is fine (most workflows use 4). For slicer, show presets 1-4 with shift paging to 5-8.

## 7-Segment Display Usage

With 2 groups of ~3 digits per deck:
- Deck layer indicator (A/b) — reuse `encode_7segment()` from F1 driver
- Loop size in beats (e.g., "4", "16", "0.5")
- BPM display (e.g., "174")
- FX preset number

## LED Feedback Mapping

### Single-Color LEDs
- Play, Cue, Sync — transport state
- Flux — slip mode active
- FX unit assign — which deck targets which FX

### RGB LEDs (pads)
- Hot cue colors (set/empty)
- Slicer preset state (assigned/active with beatgrid-synced pulse)
- Same feedback pipeline as F1 grid pads

### Touch Strip LEDs (21 segments)
- Playhead position visualization
- Loop region highlight
- FX parameter intensity display

## Implementation Plan

### Files to Create/Modify

1. **New**: `crates/mesh-midi/src/hid/devices/kontrol_x1.rs`
   - `KontrolX1` struct implementing `HidDeviceDriver`
   - Input report parser (12-bit knob values, 4-bit encoders, button bitmask, touch strip)
   - Output report builder (LED brightness, RGB pads, 7-segment, touch strip LEDs)
   - Control name constants (~50-60 controls)

2. **Modify**: `crates/mesh-midi/src/hid/mod.rs`
   - Register X1 MK2 in `KNOWN_DEVICES` (VID=0x17CC, PID=0x1220)
   - Add `kontrol_x1` module

3. **Modify**: `crates/mesh-midi/src/hid/thread.rs`
   - Add post-write handshake support (read from secondary endpoint after output write)
   - Make this opt-in per device driver (F1 doesn't need it)

4. **New**: `etc/udev/99-mesh-hid.rules` — add X1 MK2 entry:
   ```
   SUBSYSTEM=="hidraw", ATTRS{idVendor}=="17cc", ATTRS{idProduct}=="1220", MODE="0660", GROUP="plugdev"
   ```

### Implementation Order
1. Extend HID I/O thread for post-write handshake (infrastructure)
2. Basic input parsing (buttons, knobs, encoders)
3. LED output (single-color buttons, RGB pads)
4. 7-segment display (reuse F1 encoding)
5. Touch strip input + LED output
6. Phase meters
7. MIDI learn integration (X1-specific step definitions)

## Resources

- **Mixxx X1 MK2 HID mapping** (gist by stotes): Full byte-level protocol in JavaScript
- **joherold/traktor_x1** (GitHub): Rust/Python X1 communication tools
- **Mixxx discourse thread**: Community X1 MK2 mapping discussion with protocol details
- **Mixxx HID mapping wiki**: General NI HID patterns

## Notes

- 12-bit knob resolution (4096 steps) is significantly better than MIDI CC (128 steps) — excellent for precise FX macro control
- Same NI HID protocol family as F1 (VID 0x17CC, 0x00-0x7F brightness range)
- Always target MK2+ for NI hardware on Linux — MK1 versions use proprietary protocols
- Hardware testing needed to confirm: exact 7-segment byte mapping, touch strip RGB encoding (green channel unclear), phase meter segment count
