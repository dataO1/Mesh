# MIDI and HID Controller Support

Mesh works with any class-compliant MIDI controller and select USB HID devices. You can map every control through the built-in Learn wizard -- no manual YAML editing required.

This document covers controller setup, the Learn wizard workflow, layer toggle configuration, LED feedback, shift modifiers, compact mapping strategies, and the configuration file format.

---

## Supported Protocols

### MIDI

Any USB MIDI controller that sends standard messages will work. Mesh listens for:

- **Note On / Note Off** -- buttons, pads, toggle switches
- **Control Change (CC)** -- faders, knobs, encoders
- **Relative encoders** -- automatically detected during learn (values around 64 = clockwise, below 64 = counter-clockwise)

No driver installation is needed. Mesh uses the system's class-compliant MIDI stack (ALSA on Linux).

### HID

Some controllers communicate over raw USB HID instead of MIDI. Mesh includes native drivers for these devices, providing direct access to RGB LEDs, 7-segment displays, and controls that are not exposed over MIDI.

Currently supported HID devices:

- **Native Instruments Kontrol F1 MK2** -- 4x4 RGB pad grid, 4 faders, 4 knobs, encoder with push, shift button, 7-segment display

On Linux, HID devices require a udev rule so Mesh can open the device without root. The `.deb` package installs this automatically at `/etc/udev/rules.d/99-mesh-hid.rules`. For manual installation:

```
sudo cp etc/udev/99-mesh-hid.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

On NixOS, add to `configuration.nix`:

```nix
services.udev.extraRules = builtins.readFile ./99-mesh-hid.rules;
```

---

## Tested Devices

| Device | Protocol | Features |
|--------|----------|----------|
| Allen & Heath Xone K2 | MIDI | Rotary encoders, buttons, note-offset LEDs (red/amber/green) with beat-synced pulsing |
| Allen & Heath Xone K3 | MIDI | Same as K2 but with full-RGB LEDs configurable via Xone Controller Editor |
| Native Instruments Kontrol F1 MK2 | HID | 4x4 RGB pad grid, encoders, faders, full-color LED feedback, 7-segment display |
| Pioneer DDJ-SB2 | MIDI | Profile included |
| Any class-compliant MIDI controller | MIDI | Via Learn wizard |

If your controller sends standard MIDI messages, it will work. The Learn wizard auto-detects control types (button, knob, fader, encoder) from the messages your hardware sends.

---

## MIDI Learn Wizard

The Learn wizard maps every control on your hardware to a Mesh function. Start it in one of these ways:

- Launch `mesh-player` with no existing `midi.yaml` -- the wizard starts automatically
- Launch with `mesh-player --midi-learn` to force a fresh learn session
- Open Settings and press the MIDI Learn button to re-map

<!-- TODO: GIF -- Learn wizard startup, showing the initial navigation capture prompt -->

### Phase 1: Navigation Capture

The first thing you map is the **browse encoder** (rotate to scroll) and **browse select button** (press to confirm). These two controls let you navigate the rest of the wizard using your hardware, so you do not need to touch the screen after this point.

A 1-second debounce window prevents accidental double-mapping. Move a control once, then wait for the prompt to advance.

### Phase 2: Topology Setup

The wizard asks a series of questions to understand your controller layout. Answer using the browse encoder and select button you just mapped, or directly on screen:

1. **Controller name** -- Free text identifier for this device (e.g., "Xone K2 Left"). Shown in Settings.

2. **Deck count** -- Choose one:
   - **2 Decks** -- Two physical sides, two virtual decks
   - **2 Decks + Layer Toggle** -- Two physical sides, four virtual decks via a layer switch
   - **4 Decks** -- Four physical sides, four virtual decks (direct)

3. **Layer toggle button** (only if you chose 2 + Layer Toggle) -- The button that switches between Layer A and Layer B. See the [Deck Layer System](#deck-layer-system) section below.

4. **Pad mode source** -- How the pad grid determines what mode it is in:
   - **App** -- Pads always send the same MIDI notes; Mesh decides the action based on which mode is active in software (e.g., Kontrol F1)
   - **Controller** -- The controller has hardware mode buttons that change which MIDI notes the pads send (e.g., DDJ-SB2)

5. **Shift buttons** -- Up to two, one per physical side. Hold shift to access alternate functions on other buttons. If your controller has separate left/right shift buttons, map both.

6. **Toggle buttons** (optional) -- Used as layer A/B indicators with LED feedback.

### Phase 3: Tree-Based Mapping

After topology setup, a collapsible tree appears showing every mappable function. Navigate the tree with the browse encoder (scroll) and select button (expand/collapse sections, start mapping a control).

<!-- TODO: GIF -- Tree navigation with encoder, expanding a section, then mapping a control -->

When you select a mapping slot, Mesh highlights the corresponding control in the UI with a red border. Move the physical control you want to assign, and the mapping is captured.

**Mapped controls work immediately.** You can test each control as soon as it is mapped while continuing to assign others.

The tree contains the following sections. Per-deck sections repeat once per physical deck (or per virtual deck for mixer channels).

#### Navigation

| Control | Type | Description |
|---------|------|-------------|
| Shift Button -- Left | Button | Hold for shift-layer on the left side |
| Shift Button -- Right | Button | Hold for shift-layer on the right side |
| Browse Encoder | Encoder | Scroll the track browser, settings, and mapping tree |
| Browse Press | Button | Load tracks, confirm selections, open folders |
| Browser Back | Button | Navigate up one level in the browser |
| Browse Encoder 2-4 | Encoder | Additional browse encoders for multi-deck setups |

#### Transport (per physical deck)

| Control | Type | Description |
|---------|------|-------------|
| Play | Button | Start or pause playback |
| Cue | Button | Hold to preview from cue point, release to snap back |
| Loop Toggle | Button | Turn the active loop on or off |
| Loop Size Encoder | Encoder | Turn to halve or double the loop length |
| Loop In | Button | Set loop start at current position |
| Loop Out | Button | Set loop end and activate the loop |
| Beat Jump Back | Button | Jump backward by the current beat jump size |
| Beat Jump Forward | Button | Jump forward by the current beat jump size |
| Slip Mode | Button | Enable slip -- playback continues underneath loops and scratches |
| Key Match | Button | Transpose this deck's pitch to match the master deck |
| Suggestion Energy | Knob | Bias track suggestions toward higher or lower energy |
| Browser Toggle | Button | Toggle the track browser on this side |
| Deck Load | Button | Load the selected browser track into this deck |

#### Performance Pads (per physical deck)

| Control | Type | Description |
|---------|------|-------------|
| Hot Cue Mode | Button | Switch pads to hot cue mode |
| Slicer Mode | Button | Switch pads to slicer mode |
| Hot Cue 1-8 | Button | Set or trigger a cue point |
| Slicer 1-8 | Button | Trigger a slice from the slicer buffer |
| Slicer Reset | Button | Clear the slicer buffer and return to normal playback |

Hot cue and slicer pad mappings only appear when pad mode source is set to "Controller" (hardware mode buttons). When set to "App," the same pads automatically switch function based on the current software mode.

#### Stems (per physical deck)

| Control | Type | Description |
|---------|------|-------------|
| Vocals/Drums/Bass/Other Mute | Button | Silence the corresponding stem |
| Vocals/Drums/Bass/Other Solo | Button | Play only this stem, muting all others |
| Vocals/Drums/Bass/Other Link | Button | Link this stem to the same stem on the paired deck for transitions |

Stem mute buttons use the physical deck index directly (not layer-resolved). This means the 4x4 stem matrix always maps to the same four virtual decks regardless of the current layer.

#### Mixer (per virtual deck)

| Control | Type | Description |
|---------|------|-------------|
| Volume | Fader | Channel volume |
| Filter | Knob | Bipolar filter: left = low-pass, center = off, right = high-pass |
| EQ High | Knob | 3-band EQ high frequency |
| EQ Mid | Knob | 3-band EQ mid frequency |
| EQ Low | Knob | 3-band EQ low frequency |
| Cue / PFL | Button | Send this channel to the headphone cue bus |

Mixer controls are per virtual deck. With a 2-deck + layer toggle setup, four mixer channels appear in the tree (one per virtual deck).

#### Effects (per physical deck)

| Control | Type | Description |
|---------|------|-------------|
| FX Macro 1-4 | Knob | Control parameters of the active FX preset |

FX macros follow layers -- when you switch layers, the macros control the newly targeted deck's FX preset.

#### Global Controls

| Control | Type | Description |
|---------|------|-------------|
| Crossfader | Fader | Blend between left and right channels |
| FX Preset Encoder | Encoder | Scroll through available FX presets for all decks |
| FX Preset Select | Button | Apply the currently highlighted FX preset |
| Master Volume | Fader | Main output level |
| Cue Volume | Knob | Headphone output level |
| Cue Mix | Knob | Balance between cue (preview) and master in headphones |
| BPM | Knob | Adjust the global tempo |
| Settings Button | Button | Open or close the settings panel |

### Phase 4: Review

After mapping all controls, a summary screen shows every assigned mapping.

- **Save** -- Write the configuration to `midi.yaml` and exit the wizard
- **Cancel** -- Discard all changes
- **Reset All** -- Clear all mappings (with a confirmation dialog) and start over

<!-- TODO: Screenshot -- Review screen showing a complete mapping summary -->

---

## Deck Layer System

The layer system lets you control 4 virtual decks from a 2-deck controller using a toggle button.

### How Layers Work

| Layer | Physical Left | Physical Right |
|-------|:-------------:|:--------------:|
| **A** (default) | Deck 1 | Deck 2 |
| **B** (toggled) | Deck 3 | Deck 4 |

Press the mapped layer toggle button to switch between layers. All layer-resolved controls (transport, effects, browser) follow the active layer. When you press Play on the left side in Layer B, it controls Deck 3.

### What Does NOT Follow Layers

Two types of controls use fixed deck indices instead of layer resolution:

- **Stem mute/solo/link buttons** -- The 4x4 stem matrix maps directly to virtual decks. On a Kontrol F1, the 16 pads can represent 4 stems across 4 decks simultaneously, regardless of the current layer.
- **Mixer channels** -- Volume faders, EQ knobs, and filters are assigned to specific virtual decks during learn, not to physical sides. This prevents accidentally changing volume on the wrong deck when switching layers.

### Layer LED Feedback

When a layer toggle button has LED output:
- **Red** for Layer A
- **Green** for Layer B

The deck header labels in the UI also change color: red when targeted by Layer A, green when targeted by Layer B.

---

## LED Feedback

Mesh sends LED state back to your controller in real time. The type of feedback depends on your hardware.

### MIDI LED Feedback

For standard MIDI controllers, Mesh sends Note On messages with velocity values to control LED brightness. Controllers with note-offset color schemes (like the Allen & Heath Xone K series) get automatic color selection:

- **Red layer** (note offset +0) -- used for bass stem, play states
- **Amber layer** (note offset +36) -- used for drums, other stem
- **Green layer** (note offset +72) -- used for vocals stem, active states

These offsets are auto-detected when the controller name contains "xone." The actual colors displayed depend on the palette configured in the Xone Controller Editor.

### HID RGB Feedback

HID devices with full RGB LEDs receive per-control color values. The Kontrol F1 gets distinct colors for every state.

### Feedback States

| State | MIDI | HID Color | Notes |
|-------|------|-----------|-------|
| Playing | LED on | Green | Pulses to the beat when beat-synced |
| Cueing | LED on | Orange | Active while cue button is held |
| Loop active | LED on | Cyan | |
| Slip active | LED on | Amber | |
| Key match | LED on | Teal | |
| Stem muted | LED on | Per-stem color | Vocals=teal, Drums=navy, Bass=red-orange, Other=violet |
| Stem linked | LED on | Shifted hue | Distinct from mute color to differentiate linked state |
| Hot cue set | LED on | Amber | Dim when empty |
| Slicer assigned | LED on | Cyan | Shows which presets have patterns |
| Headphone cue (PFL) | LED on | Yellow | |
| Hot cue mode | LED on | Blue | Mode indicator |
| Slicer mode | LED on | Purple | Mode indicator |
| Layer A/B | LED on | Red/Green | Via alt_on_value in config |
| Browse mode | LED on | White | Active when browser is open on this side |

### Beat-Synced Pulsing

On devices that support it, LEDs pulse in time with the master deck's beat grid. The pulse uses a cosine curve:

- **HID devices** -- Smooth brightness breathing between 15% and 100%. Never fully off.
- **MIDI devices** -- Full on/off pulsing (reaches velocity 0 at mid-beat) so binary LEDs visibly blink.

Beat phase comes from the master deck's beat grid, so the pulse stays locked to the music.

<!-- TODO: GIF -- Kontrol F1 LEDs pulsing to the beat with stem colors visible -->

---

## Shift Functionality

Shift buttons are long-press modifiers that unlock alternate functions on other controls.

### Per-Physical-Deck Shift

Each shift button is tied to a physical controller side:

- **Left shift** only affects controls on the left physical side
- **Right shift** only affects controls on the right physical side

This prevents crosstalk when two hands are working independently.

### Global Shift

If a mapping has no `physical_deck` assigned (global controls like FX encoder, master volume), pressing any shift button activates the shift action.

### Common Shift Actions

| Normal Action | Shift Action |
|--------------|--------------|
| Browse select | Browser back (navigate up) |
| Hot cue trigger | Set / delete hot cue |
| Slicer pad | Queue slice for playback |

The specific shift bindings depend on your mapping. During the Learn wizard, after mapping a control's primary action, you can optionally map a shift action for it.

---

## Compact 4-Deck Mapping (Momentary Mode)

When your controller has limited buttons but you want access to hot cues, slicer, and stem controls, use momentary mode overlays.

### How It Works

1. Set `pad_mode_source` to **App** during topology setup
2. Map **Hot Cue Mode** and **Slicer Mode** buttons in the Performance Pads section

With momentary mode enabled (`momentary_mode_buttons: true` in the config):

- **Hold** the Hot Cue Mode button -- pads temporarily switch to hot cue triggers
- **Hold** the Slicer Mode button -- pads temporarily switch to slicer pads
- **Release** -- pads return to their default performance mode (stem mutes/transport)

This is particularly useful with two Kontrol F1 units:

- Left F1 controls decks 1 and 3 (layer A/B)
- Right F1 controls decks 2 and 4 (layer A/B)
- 16 pads default to stem mutes + transport, hold mode button for hot cues or slicer
- Each F1 has its own browse encoder for independent deck browsing

### Per-Side Mode Buttons

Mode buttons are per physical side:
- Left mode button affects the pads on the left controller side (decks 1+3 via layer)
- Right mode button affects the pads on the right controller side (decks 2+4 via layer)

### Dual Browse Encoders

You can map up to 4 independent browse encoders (Browse Encoder 1-4) in the Navigation section. Each encoder scrolls and selects tracks for a different deck, allowing two people to browse simultaneously.

---

## Configuration File

Mappings are saved to `~/Music/mesh-collection/midi.yaml`.

This file is generated by the Learn wizard. You rarely need to edit it by hand, but understanding the format can be helpful for fine-tuning.

### File Structure

```yaml
devices:
  - name: "My Controller"           # Human-readable name
    port_match: "controller name"    # Substring matched against MIDI port name
    learned_port_name: "Full Port Name"  # Exact port name from learn session
    device_type: hid                 # For HID devices: driver identifier
    hid_product_match: "Product"     # USB product name match for HID
    hid_device_id: "SERIALNUM"       # USB serial for multi-device setups
    momentary_mode_buttons: true     # Hold-to-activate pad mode overlay

    deck_target:                     # Deck routing
      type: Layer                    # "Direct" or "Layer"
      toggle_left: { ... }          # Layer toggle button (Layer mode only)
      toggle_right: { ... }
      layer_a: [0, 1]              # Virtual deck indices for Layer A
      layer_b: [2, 3]              # Virtual deck indices for Layer B

    pad_mode_source: app             # "app" or "controller"

    shift_buttons:                   # Per-physical-deck shift buttons
      - control: { ... }
        physical_deck: 0             # 0 = left, 1 = right

    mappings:                        # Control-to-action mappings
      - control:
          protocol: midi
          type: control_change
          channel: 0
          cc: 7
        action: mixer.volume
        physical_deck: 0             # Layer-resolved deck
        deck_index: null             # Direct deck (non-layer-resolved)
        behavior: continuous         # momentary | toggle | continuous
        shift_action: null           # Alternate action when shift held
        encoder_mode: absolute       # absolute | relative | relative_signed
        hardware_type: fader         # button | knob | fader | encoder
        mode: null                   # hot_cue | slicer | null (always active)

    feedback:                        # LED feedback mappings
      - state: deck.is_playing       # State to monitor
        physical_deck: 0
        output:
          protocol: midi
          type: note
          channel: 0
          note: 11
        on_value: 127                # Velocity when active
        off_value: 0                 # Velocity when inactive
        alt_on_value: 64             # Layer B active value (for layer LEDs)
        on_color: [0, 200, 0]        # RGB for HID devices
        off_color: [0, 0, 0]
```

### Control Address Formats

MIDI controls use channel + note/CC number:

```yaml
# Note message
control:
  protocol: midi
  type: note
  channel: 0
  note: 36

# CC message
control:
  protocol: midi
  type: control_change
  channel: 0
  cc: 7
```

HID controls use a device ID and named control:

```yaml
control:
  protocol: hid
  device_id: "B2220F4E"
  name: "grid_1"
```

### Multiple Devices

Multiple controllers can be listed under `devices:`. Each device profile is matched independently. Two identical controllers (e.g., two Kontrol F1 units) are distinguished by their USB serial number (`hid_device_id`).

### Re-mapping

To start fresh:

1. Delete `~/Music/mesh-collection/midi.yaml`
2. Restart `mesh-player` -- the Learn wizard starts automatically

Or open Settings and press the MIDI Learn button to re-map without deleting the file.

---

## Troubleshooting

### Controller Not Detected

- **Check USB connection.** Unplug and replug the controller.
- **Linux: check user group.** Your user should be in the `audio` group for MIDI access: `groups $USER` should include `audio`.
- **HID devices: check udev rules.** Run `ls -l /dev/hidraw*` to verify the device node exists. If permissions are denied, install the udev rules file (see [HID section above](#hid)).
- **Multiple identical controllers:** Mesh distinguishes them by USB serial number. If detection fails, check `hid_device_id` in `midi.yaml` against the actual serial reported in the log.

### Double-Mapping During Learn

The Learn wizard has a 1-second debounce window. If a control registers twice:

- Move the control once (a single button press or a short encoder turn), then wait for the prompt to advance.
- Avoid holding buttons or spinning encoders continuously during capture.

### LEDs Not Working

- **Check output port.** Some controllers use a separate MIDI port for LED output. Mesh tries to match output ports by name.
- **Check MIDI channel.** LED feedback is sent on the same channel as the input mapping. If your controller expects a different output channel, you may need to edit the `feedback` section in `midi.yaml`.
- **Note-offset LED controllers (Xone K):** Mesh auto-detects controllers with "xone" in the name. If your controller uses a similar scheme with a different name, add `color_note_offsets` to the device profile:

```yaml
color_note_offsets:
  red: 0
  amber: 36
  green: 72
```

### Encoder Direction Reversed

Re-learn the control. Mesh detects the rotation direction from the first turn during the Learn capture. If you turned it the wrong way, delete the mapping and assign it again.

### HID Device Not Found After System Update

The udev rules file may have been removed during a system update. Reinstall it:

```
sudo cp /path/to/99-mesh-hid.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then unplug and replug the controller.

### Checking Logs

Mesh logs MIDI/HID activity at startup and during operation. Run with verbose logging to diagnose issues:

```
RUST_LOG=mesh_midi=debug mesh-player
```

Look for lines starting with `HID:` or `MIDI:` for device discovery, connection, and mapping activity.
