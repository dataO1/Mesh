# Configuration Reference

Mesh stores all configuration in YAML files. Every setting described here can also
be changed through the Settings modal in the application UI.

---

## Config File Locations

| File | Linux | Windows |
|------|-------|---------|
| Collection root | `~/Music/mesh-collection/` | `~\Music\mesh-collection\` |
| Player config | `<collection>/config.yaml` | `<collection>/config.yaml` |
| Theme definitions | `<collection>/theme.yaml` | `<collection>/theme.yaml` |
| Slicer presets | `<collection>/slicer.yaml` | `<collection>/slicer.yaml` |
| MIDI mapping | `~/.config/mesh-player/midi.yaml` | *(generated via MIDI Learn)* |

All config files use YAML format. If a file is missing, Mesh creates it with
sensible defaults on first launch.

---

## Settings Modal (mesh-player)

Open the Settings modal from the header bar during a performance session. The
modal is organized into the sections listed below.

### Recording

| Setting | Description |
|---------|-------------|
| Record Set / Stop Recording | Start or stop recording the master output to a WAV file. A confirmation dialog appears before recording begins. The recording is written to every connected USB stick that contains a Mesh database. If no USB stick is connected, the recording is saved to the local collection. |

### Power (embedded only)

These options appear only when Mesh is running on a NixOS-based embedded device.

| Setting | Description |
|---------|-------------|
| Power Off | Safely shut down the device. A confirmation dialog is shown before proceeding. |

### Audio Output

| Setting | Description |
|---------|-------------|
| Master device | Select which stereo pair to use for main speaker output. |
| Cue device | Select which stereo pair to use for headphone monitoring. |
| Refresh Devices | Re-scan available audio hardware. Use this after plugging in or unplugging an interface. |

### Playback

| Setting | Description |
|---------|-------------|
| Automatic Beat Sync (Phase Sync) | Toggle automatic phase alignment when pressing play. When enabled, tracks snap to the global beat grid so that beats stay locked across decks. |
| Default Loop Length | Choose the default loop size when activating a loop. Options: 1/8, 1/4, 1/2, 1, 2, 4, 8, 16, 32, 64, 128, 256 beats. |

### Display

| Setting | Description |
|---------|-------------|
| Waveform Layout | **Horizontal** (default) -- time runs left to right. **Vertical** -- time runs top to bottom. **Vertical Inverted** -- time runs bottom to top. Changes take effect immediately. |
| Waveform Abstraction | **Low** -- maximum detail, shows every peak. **Medium** (default) -- balanced detail and clarity. **High** -- smoothed waveform with less visual noise. Controls the peak subsampling grid resolution. |
| Default Zoom Level | Number of bars visible in the zoomed waveform view. Options: 2, 4, 8, 16, 32, 64 bars. |
| Overview Grid Density | Spacing of major grid lines on the overview waveform. Options: 8, 16, 32, 64 beats between lines. |
| Theme | Select from the themes defined in `theme.yaml`. The change previews live and does not require a restart. |
| Font | **Exo** (default, geometric sans-serif), **Hack** (monospace), **JetBrains Mono** (monospace), or **Press Start 2P** (retro 8-bit pixel font). Requires a restart to take effect. |
| Font Size | **Small** (90%), **Medium** (100%, default), or **Big** (110%). Requires a restart to take effect. |
| Show Local Collection | Toggle whether the local on-disk collection appears in the browser sidebar. Turn this off when using a USB-only workflow on an embedded device. |
| Persistent Browse | When enabled, the browser overlay stays visible instead of automatically hiding after 5 seconds of inactivity. |

### Key Matching

| Setting | Description |
|---------|-------------|
| Key Scoring Model | **Camelot** -- the classic DJ key wheel used in most DJ software. **Krumhansl** -- a perceptual music psychology model that measures tonal similarity. This affects the smart suggestion ranking and the harmonic compatibility indicators throughout the interface. |

### Loudness

| Setting | Description |
|---------|-------------|
| Auto-Gain Normalization | Toggle automatic volume matching. When enabled, each track's gain is adjusted based on its measured LUFS loudness so that all tracks play at a consistent perceived volume. |
| Target Loudness | The reference level that tracks are normalized to. Options: **-6 LUFS** (Loud), **-9 LUFS** (Medium), **-14 LUFS** (Streaming, default), **-16 LUFS** (Broadcast). |

### Slicer

| Setting | Description |
|---------|-------------|
| Buffer Size | How much audio the slicer captures. Options: 1, 4, 8, 16 bars. Larger buffers give you more material to slice from but use more memory. Slicer presets themselves are edited in mesh-cue. |

### Network (Linux only)

Network settings appear only on Linux, where Mesh uses NetworkManager via D-Bus.

| Setting | Description |
|---------|-------------|
| WiFi status | Displays the current SSID, signal strength, and connection state. |
| Scan and connect | Scan for nearby WiFi networks and connect. On embedded devices, passwords are entered via the on-screen keyboard. |
| Disconnect | Disconnect from the current WiFi network. |
| LAN status | Displays wired Ethernet connection state. |

### System Update (NixOS embedded only)

These options appear only when Mesh is running on a NixOS-based embedded device.

| Setting | Description |
|---------|-------------|
| Current version | Shows the installed version number. |
| Check for Update | Queries the GitHub releases API for the latest available version. |
| Install Update | Downloads and installs the update by triggering a NixOS system rebuild via a systemd service. Progress is shown by polling the systemd journal. |
| Pre-release Updates | When enabled, release candidates and beta versions are included in update checks. |
| Restart | Restarts the cage compositor to launch the newly installed version after an update completes. |

### MIDI Controller

| Setting | Description |
|---------|-------------|
| Start MIDI Learn | Enter the MIDI learn wizard. The wizard walks through a guided series of phases (Setup, Transport, Pads, Stems, Mixer, Browser, Review) to map every control on your MIDI controller. The resulting mapping is saved to `midi.yaml`. |

---

## Settings in mesh-cue

mesh-cue is the editing and preparation application. Its settings are accessed
from the Settings panel within the application.

### Audio Output

| Setting | Description |
|---------|-------------|
| Output device | Select the audio output device for preview playback. |
| Scratch interpolation | Method used for audio interpolation during scratching. **Linear** (fast, lowest CPU), **Cubic** (smooth, good balance), **Sinc** (highest quality, most CPU). |

### Analysis

| Setting | Description |
|---------|-------------|
| BPM Detection Range (Min) | Lower bound for tempo detection. Range: 40--180 BPM. Set this to match the slowest tempo you expect in your library. |
| BPM Detection Range (Max) | Upper bound for tempo detection. Range: 60--250 BPM. Set this to match the fastest tempo you expect. |
| BPM Source | **Drums Only** (recommended) -- analyzes the isolated drum stem for more accurate results in electronic music. **Full Mix** -- analyzes the full audio signal. |
| Beat Detection Method | **Simple** (Essentia) -- fast traditional algorithm. **Advanced** (Beat This! ML model) -- more accurate neural network approach that also detects downbeats. Uses more CPU during analysis. |
| Parallel Processes | Number of tracks analyzed simultaneously during import. Range: 1--16. Higher values speed up batch imports but use more CPU and RAM. |

### Display

| Setting | Description |
|---------|-------------|
| Overview Grid Density | Spacing of major grid lines on the overview waveform. Options: 8, 16, 32, 64 beats. |
| Slicer Buffer Size | Buffer length for the slicer preview. Options: 4, 16, 32, 64 beats. |
| Theme | Select from the themes defined in `theme.yaml`. |
| Font | Font selection (same options as mesh-player). Requires a restart. |

### Track Name Format

| Setting | Description |
|---------|-------------|
| Template | A format string that controls how track names are displayed. Use `{artist}` and `{name}` as placeholders. Default: `{artist} - {name}`. |

### Separation

These settings control stem separation when importing audio in Mixed Audio mode.

| Setting | Description |
|---------|-------------|
| Backend | ONNX Runtime (the only available backend). |
| Model | **Demucs 4-stem** -- standard separation model. **Demucs 4-stem Fine-tuned** -- slightly better separation quality. |
| GPU Acceleration | When enabled, Mesh attempts to use the GPU for separation. Falls back to CPU automatically if no compatible GPU is found. |
| Segment Length | Length of each audio segment processed at a time, in seconds. Range: 5--60. Shorter segments use less RAM but may introduce minor artifacts at boundaries. |
| Shifts | Number of offset passes for improved separation quality. **Off** (1x), **Low** (2x), **Medium** (3x), **High** (4x), **Maximum** (5x). More shifts produce cleaner stems but take proportionally longer. |

---

## Theme Customization

Themes are defined in the `theme.yaml` file inside your collection root. You can
define multiple themes in the same file and switch between them from the Display
section of Settings. Changes apply instantly without a restart.

### Built-in Themes

Mesh ships with five built-in themes:

- **Mesh** (default) -- a Gruvbox-inspired warm dark palette
- **Catppuccin** -- soft pastel tones on a dark background
- **Rose Pine** -- purple and blue accents
- **Synthwave** -- neon colors on a dark background
- **Gruvbox** -- the classic Gruvbox dark color scheme

### Creating a Custom Theme

Add a new entry to `theme.yaml`. Each theme requires a name, six interface
colors, and four stem colors:

```yaml
- name: "My Theme"
  background: "#1d2021"
  text: "#ebdbb2"
  accent: "#b8bb26"
  success: "#98971a"
  warning: "#d79921"
  danger: "#cc241d"
  stems:
    - "#33CC66"    # Vocals (green)
    - "#CC3333"    # Drums (red)
    - "#E6604D"    # Bass (orange)
    - "#00CCCC"    # Other (cyan)
```

**Color fields:**

| Field | Purpose |
|-------|---------|
| `background` | Main background color for all panels. |
| `text` | Primary text color. |
| `accent` | Highlight color for selected items, active controls, and focused elements. |
| `success` | Color for positive indicators (connected status, successful operations). |
| `warning` | Color for caution indicators (pending operations, threshold warnings). |
| `danger` | Color for destructive actions and error states. |
| `stems` | Array of exactly four hex colors, one per stem in order: Vocals, Drums, Bass, Other. |

Stem colors affect waveform rendering, stem mute buttons, and MIDI controller
LED feedback.

If `theme.yaml` is missing or contains parse errors, Mesh falls back to the
built-in default theme.

---

## Audio Device Setup

### Linux with JACK

Mesh automatically connects to JACK when it is available (including through
PipeWire's JACK bridge, which is used transparently via `pw-jack`).

- Master and Cue outputs can be routed to specific JACK port pairs. For example,
  on a Scarlett interface you might route Master to ports 1--2 and Cue to ports
  3--4.
- Audio devices can be hot-swapped without restarting Mesh when running under
  JACK.

### Linux with CPAL (fallback)

When JACK is not available, Mesh falls back to CPAL (Cross-Platform Audio
Library).

- Devices are selected by stereo pair index.
- Changing audio devices may require restarting the application.

### Windows with WASAPI

- Devices are selected by name from a dropdown list.
- Master and Cue can be assigned to different audio interfaces.
- Audio device changes are not hot-swappable. Restart Mesh after plugging in or
  removing an audio interface.

### Headphone Monitoring

To use headphone cueing effectively:

1. Set Master and Cue to **different** output devices, or to different channel
   pairs on the same multi-output interface (for example, outputs 1--2 for
   Master and outputs 3--4 for Cue on a 4-output interface).
2. Use the Cue Volume and Cue/Master Mix controls in the mixer section to blend
   between the cued track and the master output in your headphones.

If Master and Cue are set to the same device and channel pair, headphone
monitoring will not function as intended because both signals are mixed to the
same output.

---

## Command-Line Arguments

### mesh-player

| Argument | Description |
|----------|-------------|
| *(no arguments)* | Start in performance mode. If no `midi.yaml` file exists, the MIDI learn wizard starts automatically. |
| `--midi-learn` | Start directly in MIDI learn mode to create or redo a controller mapping. |

### mesh-cue

mesh-cue is a GUI-only application and does not accept command-line arguments.
