# Embedded Mesh Player Setup Guide

Run mesh-player as a standalone embedded DJ unit on an ARM64 single-board computer with no laptop required.

## Overview

```
Orange Pi 5 Pro (89×56mm, $80)
├── I2S0 → ES8388 codec → 3.5mm jack → Headphones (CUE)
├── I2S3 → PCM5102A DAC → 3.5mm out  → PA System (MASTER)
├── HDMI → 7" touchscreen (or any HDMI display)
├── USB  → DJ's USB stick (track library)
└── USB  → MIDI controller (optional)

NixOS boots → cage kiosk → mesh-player fullscreen (~10s cold boot)
```

The entire system costs ~$112 for the core components (board + boot media + DAC + power supply + HDMI cable). A fully enclosed unit with case and cooling runs ~$165.

## Hardware Requirements

### Board: Orange Pi 5 Pro 8GB (primary target)

| Spec | Detail |
|------|--------|
| SoC | RK3588S (4x A76 @2.4GHz + 4x A55 @1.8GHz) |
| GPU | Mali-G610 MP4 (Vulkan 1.2, GLES 3.2) |
| RAM | 8GB LPDDR5 (sufficient — worst case 3.7 GB used) |
| Storage | microSD (OS boot), USB 3.0 (tracks) |
| WiFi | Built-in WiFi 5 + BT 5.0 |
| Audio | ES8388 codec + I2S3 on 40-pin GPIO |
| Size | 89×56mm (credit-card) |
| Power | USB-C 5V/5A (~6-12W) |
| Price | ~$80 |

Other compatible boards (same audio architecture): Orange Pi 5 Max ($145, WiFi 6E, PCIe 3.0 NVMe), Orange Pi 5 Plus ($142, dual 2.5GbE).

### Bill of Materials

**Core (required):**

| Component | Spec | Price |
|-----------|------|-------|
| Orange Pi 5 Pro 8GB | RK3588S, LPDDR5, WiFi 5, BT 5.0 | $80 |
| microSD card | 32GB A2 U3 (OS boot) | $8 |
| GY-PCM5102 I2S DAC | PCM5102A, 112 dB SNR | $5 |
| Dupont jumper wires | Female-to-female, 6 pcs | $1 |
| USB-C PSU | 5V/5A Type-C | $12 |
| Micro-HDMI cable | 15-30cm | $6 |
| **Total** | | **$112** |

**Enclosure (recommended):**

| Component | Spec | Price |
|-----------|------|-------|
| Aluminum project box | ~150×120×50mm | $25 |
| M2.5 standoff kit | Brass, board mount | $5 |
| 40mm Noctua fan | 5V PWM | $12 |
| Thermal pad | SoC heatsink contact | $3 |
| Panel-mount 3.5mm (x2) | Master out + cue out | $4 |
| Panel-mount USB-A | MIDI controller / USB stick pass-through | $4 |
| **Total** | | **$53** |

**Optional:**

| Component | Spec | Price |
|-----------|------|-------|
| NVMe SSD | M.2 2280 (built-in track library) | ~$40-55 |
| 7" IPS touchscreen | 1024×600, HDMI + USB touch | ~$40 |
| Powered USB 3.0 hub | 4-port | ~$15 |

## I2S DAC: How It Works

The PCM5102A DAC connects to the Orange Pi's 40-pin GPIO header via 6 jumper wires. No soldering required for prototyping.

### Wiring Diagram

```
Orange Pi 5 Pro                    GY-PCM5102 Breakout
40-pin GPIO Header                 (PCM5102A DAC)
┌──────────────┐                   ┌──────────────┐
│ Pin 1  (3.3V)│───── red ────────▶│ VIN          │
│ Pin 6  (GND) │───── black ──────▶│ GND          │
│              │                   │ SCK ◀── GND  │  (tie SCK to GND pad on board)
│ Pin 35 (SCLK)│───── yellow ─────▶│ BCK          │
│ Pin 38 (LRCK)│───── green ──────▶│ LRCK         │
│ Pin 40 (SDO) │───── blue ───────▶│ DIN          │
└──────────────┘                   └──────┬───────┘
                                          │ 3.5mm jack
                                          ▼
                                    PA System / Mixer
                                    (MASTER output)
```

### Pin Reference

| PCM5102A Pin | 40-Pin Header | GPIO | Signal |
|---|---|---|---|
| BCK | Pin 35 | GPIO3_C2 | I2S3_SCLK (bit clock) |
| LRCK | Pin 38 | GPIO3_C0 | I2S3_LRCK_TX (L/R word select) |
| DIN | Pin 40 | GPIO3_B7 | I2S3_SDO (serial audio data) |
| SCK | Tie to GND | — | Internal PLL mode |
| VIN | Pin 1 | — | 3.3V power |
| GND | Pin 6 | — | Ground |

### How It Works

1. The RK3588S's I2S3 controller generates three signals: a bit clock (BCK), a word select clock (LRCK, alternates L/R channel), and serial data (SDO — the audio samples)
2. These are 3.3V CMOS logic signals, clocked at BCK = sample_rate x bits x 2 channels (e.g., 44.1kHz x 32 x 2 = 2.822 MHz)
3. The PCM5102A's internal PLL regenerates a master clock from BCK — SCK tied to GND tells the chip to use internal PLL mode
4. The DAC converts the I2S stream to analog audio on its 3.5mm output jack
5. No I2C bus needed — the PCM5102A is a "dumb" DAC that converts whatever I2S data it receives

For production, replace jumper wires with soldered connections. The GY-PCM5102 board (30x20mm) mounts inside the enclosure with double-sided tape or M2 standoffs.

### Device Tree Overlay

Linux needs a Device Tree Overlay to enable the I2S3 controller and register the DAC as a sound card. The overlay source lives at `nix/embedded/pcm5102a-i2s3.dts` and is loaded automatically by NixOS via `hardware.deviceTree.overlays`.

For non-NixOS systems (Armbian, etc.), compile and install manually:

```bash
dtc -I dts -O dtb -o pcm5102a-i2s3.dtbo nix/embedded/pcm5102a-i2s3.dts
sudo cp pcm5102a-i2s3.dtbo /boot/dtb/rockchip/overlay/
sudo orangepi-config  # System → Hardware → enable overlay
```

After reboot, `aplay -l` shows two sound cards:

```
card 0: rockchipes8388 [rockchip-es8388]    <- onboard codec (3.5mm jack, CUE)
card 1: PCM5102A [PCM5102A]                 <- I2S DAC on GPIO (MASTER)
```

## Audio Routing

mesh-player uses cpal (cross-platform audio library) which talks to ALSA on Linux. With two sound cards, there are two routing approaches:

### Option A: Direct ALSA (recommended for production)

mesh-player opens two separate ALSA devices directly:

```
Master output → ALSA device "hw:1,0" (PCM5102A)    → PA system
Cue output    → ALSA device "hw:0,0" (ES8388)       → Headphones
```

cpal enumerates all ALSA devices — mesh-player picks the right one by matching the card name substring (`"PCM5102A"` for master, `"es8388"` for cue). Card numbering can change between boots — always match by name, never by number.

Named ALSA aliases provide stable device names regardless of card numbering (configured in `nix/embedded/audio.nix`):

```
pcm.mesh_master { type hw; card "PCM5102A"; device 0; }
pcm.mesh_cue    { type hw; card "rockchipes8388"; device 0; }
```

### Option B: PipeWire routing (for development/debugging)

PipeWire exposes both cards as sinks with runtime re-routing via `pavucontrol` or `pw-link`. Adds ~2-5ms latency.

## Audio Output Quality

### Master Output (PCM5102A I2S DAC) — Excellent

The PCM5102A is a dedicated audio DAC with 112 dB SNR, -93 dB THD+N, and no analog output stage compromises. It connects via I2S (direct digital bus, no USB overhead) and runs from the board's 3.3V rail with its own internal voltage regulators. Audio quality is comparable to mid-range professional interfaces.

### Headphone/Cue Output (ES8388 Onboard Codec) — Adequate for Cueing

The ES8388 codec on the Orange Pi 5 Pro provides the 3.5mm TRRS headphone jack. While the chip itself specs 96 dB SNR and -83 dB THD+N, the board-level implementation introduces limitations:

**Bass roll-off from coupling capacitors.** The headphone output is AC-coupled through small electrolytic capacitors on the PCB (typically 22µF in 0603 packages on Orange Pi boards). These form a high-pass filter with the headphone impedance: `f_c = 1 / (2π × C × Z)`. With typical 32Ω DJ headphones, the -3dB point is ~220 Hz — noticeable bass loss. Higher impedance headphones (250Ω) push this down to ~29 Hz (inaudible), but most DJ headphones are low-impedance.

**Noise from shared power rail.** The ES8388's analog section shares a power domain with the RK3588S SoC, which is a high-power digital processor. Digital switching noise couples into the analog output, raising the effective noise floor above the chip's datasheet spec.

**No software processing is applied.** The `mesh-audio-init` service (`audio.nix`) sets clean defaults: 3D processing disabled, mixer paths enabled, PCM volume at 85% (headroom to avoid clipping). PipeWire passes audio through without resampling at the native 48kHz rate.

### ALSA Mixer Defaults (ES8388)

Set by `mesh-audio-init.service` on every boot:

| Control | Value | Purpose |
|---------|-------|---------|
| Headphone | on | Enable headphone output path |
| hp switch | on | Route DAC to headphone amp |
| PCM | 85% | DAC digital volume (conservative headroom) |
| Output 1 / Output 2 | 100% | Analog output gain (max) |
| 3D Mode | No 3D | Disable stereo enhancement DSP |
| Left/Right Mixer | on | Enable L/R signal paths |

To adjust headphone volume: `amixer -c rockchipes8388 set PCM 90%`
To inspect all controls: `amixer -c rockchipes8388 contents`

### Upgrading the Headphone Output

If the ES8388 headphone quality is insufficient (bass-light, noisy), options in order of cost:

1. **Use higher impedance headphones** ($0) — 150-250Ω headphones shift the coupling cap roll-off well below audible range. Most studio monitoring headphones (Beyerdynamic DT 770 250Ω, Sennheiser HD 600 300Ω) work well.

2. **USB DAC dongle** (~$15) — An Apple USB-C headphone adapter (Cirrus Logic CS43131, 112 dB SNR) or similar USB dongle provides dramatically better headphone output than the onboard codec. Appears as a standard USB Audio Class device, auto-detected by ALSA.

3. **Dedicated USB audio interface** (~$65+) — A Behringer UMC204HD or similar provides 4 independent channels (2 master + 2 cue) on one device, eliminating both the I2S DAC and onboard codec. This was the original plan before the I2S DAC approach proved viable. See [ARM64 Embedded Research](arm64-embedded-research.md) for interface comparison.

## Build and Deploy Architecture

The embedded setup uses a zero-cost CI pipeline. No host system changes, no emulation, no paid services.

```
Developer pushes tag v0.9.0
          │
          ▼
GitHub Actions (ubuntu-24.04-arm)         ← free native aarch64 runner
  ├── Job 1: Build mesh-player (native ARM, no cross-compile)
  │   ├── nix copy --to file://cache (signed binary cache)
  │   └── Deploy cache to GitHub Pages
  ├── Job 2: Build SD card image (hash-deduplicated)
  │   ├── nix eval .#sdImage.drvPath → derivation hash
  │   ├── Skip if release with that hash exists
  │   └── Upload .img.zst to GitHub Releases
          │
          ▼
GitHub Pages (https://datao1.github.io/Mesh/)    ← binary cache (free)
  ├── nix-cache-info
  ├── <hash>.narinfo
  └── nar/<hash>.nar.xz
GitHub Releases                                   ← SD images (free)
  └── sdimage-<hash> → nixos-sd-image-*.img.zst
          │
          ▼
Orange Pi 5 Pro (NixOS)
  ├── Standard packages → cache.nixos.org (already cached for aarch64)
  ├── mesh-player + essentia → GitHub Pages (pre-built by CI)
  └── nixos-rebuild switch → download NARs, zero compilation
```

**Total cost: $0.** GitHub Actions ARM runners are free for public repos. GitHub Pages is free. cache.nixos.org is free.

### How Nix Binary Caches Work

A Nix binary cache is just static files served over HTTP:

| File | Content |
|------|---------|
| `nix-cache-info` | 3-line metadata (store dir, priority) |
| `<hash>.narinfo` | Per-package metadata: store path, dependencies, signature |
| `nar/<hash>.nar.xz` | Compressed binary archive (NAR = Nix ARchive) |

The device checks each package hash against the cache. If found, it downloads the pre-built NAR. If not, it would compile from source — but with CI publishing every release, the device never compiles.

### Cache Signing

Nix binary caches use Ed25519 signatures. Generate a keypair once:

```bash
nix-store --generate-binary-cache-key mesh-embedded cache-priv-key.pem cache-pub-key.pem
```

- `cache-priv-key.pem` → store in GitHub Secrets as `NIX_CACHE_PRIV_KEY`
- `cache-pub-key.pem` → configure on the device in `nix.settings.trusted-public-keys`

## NixOS Setup

### Flake Structure

All NixOS modules live in the mesh repository:

```
mesh/
├── flake.nix                        # nixosConfigurations.mesh-embedded
├── .github/workflows/
│   └── embedded-aarch64.yml         # CI: build + publish binary cache
└── nix/
    ├── common.nix                   # Shared build deps (essentia, etc.)
    ├── packages/
    │   ├── mesh-build.nix           # Full build (player + cue)
    │   └── mesh-player.nix          # Player only (embedded)
    └── embedded/
        ├── configuration.nix        # Base system config + binary cache
        ├── hardware.nix             # RK3588S: DT overlay, GPU, fast boot
        ├── audio.nix                # PipeWire + ALSA dual-card config
        ├── kiosk.nix                # cage compositor + update service
        └── pcm5102a-i2s3.dts        # I2S DAC device tree overlay
```

The flake defines `nixosConfigurations.mesh-embedded` with native aarch64 builds (no cross-compilation):

```nix
# Target platform — built natively on aarch64 CI runner
nixpkgs.hostPlatform.system = "aarch64-linux";
# buildPlatform defaults to evaluating machine's arch (aarch64 on CI)
```

### Step 1: Flash U-Boot (One-Time)

The OPi 5 Pro has SPI NOR flash for the bootloader. Flash it once using an Armbian SD card:

```bash
# Download Armbian for Orange Pi 5 (RK3588S images work for OPi 5 Pro)
wget https://dl.armbian.com/orangepi5/Bookworm_current

# Flash to microSD
dd if=Armbian_*.img of=/dev/sdX bs=1M status=progress

# Boot the OPi 5 Pro from the microSD, then:
sudo armbian-install
# Select: "Install/Update the bootloader on SPI Flash"

# Power off, remove the Armbian SD card
```

### Step 2: Get NixOS SD Image

The SD image is built by CI and uploaded to GitHub Releases. Download the latest:

```bash
# Download from GitHub Releases (look for "SD Image" releases)
gh release list --repo dataO1/Mesh | grep sdimage
gh release download sdimage-<hash> --repo dataO1/Mesh --dir /tmp

# Flash to microSD (replace /dev/sdX with your device)
zstdcat /tmp/nixos-sd-image-*.img.zst | sudo dd of=/dev/sdX bs=4M status=progress
```

The image is only rebuilt by CI when the NixOS configuration changes (hash-based deduplication). Rust-only changes update through the binary cache, not the SD image.

### Step 3: First Boot

Insert microSD, connect HDMI + USB keyboard, power on. NixOS boots into cage kiosk with mesh-player fullscreen. SSH is enabled.

```bash
ssh mesh@<board-ip>
# Default password: mesh
```

### Step 4: Configure WiFi

```bash
ssh mesh@<board-ip>
sudo nmcli device wifi connect "SSID" password "password"
```

The device now has internet access for pulling updates from GitHub Pages and cache.nixos.org.

## Deploying Updates

### Via CI (Production)

Push a version tag to trigger the CI build:

```bash
git tag v0.9.0
git push origin v0.9.0
```

GitHub Actions builds mesh-player on a native ARM runner and publishes the binary cache to GitHub Pages. The device can then update:

```bash
# SSH into the device and run:
sudo nixos-rebuild switch \
  --flake github:dataO1/Mesh/v0.9.0#mesh-embedded \
  --no-write-lock-file
```

Pre-built packages download from GitHub Pages. Standard NixOS packages download from cache.nixos.org. Zero compilation on the device.

### Via SSH (Development)

For development iteration, deploy directly from your workstation:

```bash
nixos-rebuild switch --fast \
  --flake .#mesh-embedded \
  --target-host mesh@<board-ip> \
  --use-remote-sudo
```

The `--fast` flag is required for cross-platform deployment (prevents nixos-rebuild from trying to execute aarch64 binaries on x86).

### Rollback

```bash
# On the device:
sudo nixos-rebuild switch --rollback

# Or remotely:
nixos-rebuild switch --fast --rollback \
  --target-host mesh@<board-ip> \
  --use-remote-sudo
```

NixOS atomic updates mean old generations are preserved, no partial updates occur, and the board is never in a broken state — even power loss during update is safe.

### Future: Update Button in mesh-player

The kiosk module includes a `mesh-update` systemd service with polkit rules allowing the `mesh` user to trigger it. The future update flow:

1. mesh-player checks GitHub API for latest release tag
2. UI shows "Update available: v0.9.0"
3. User clicks "Update"
4. mesh-player writes target version to `/var/lib/mesh/update-target`
5. Triggers `mesh-update.service` via D-Bus
6. `nixos-rebuild switch` runs, downloads pre-built packages, activates
7. cage restarts with the new mesh-player

## Development Workflow

```
  x86 workstation                          OPi 5 Pro
  ┌──────────────┐                         ┌──────────────────────┐
  │ Edit Rust code│                         │ NixOS running        │
  │ in mesh repo  │                         │ cage → mesh-player   │
  │               │                         │                      │
  │ nixos-rebuild │──── SSH ───────────────▶│ switch-to-config     │
  │ --fast        │     (copy store paths)  │ restart cage service │
  │ --target-host │                         │ mesh-player starts   │
  │               │◀─── SSH ────────────────│ journalctl -f output │
  │ See logs      │     (debug)             │                      │
  └──────────────┘                         └──────────────────────┘
```

1. Edit Rust code on x86 workstation
2. `nixos-rebuild switch --fast --flake .#mesh-embedded --target-host mesh@<ip> --use-remote-sudo`
3. Cross-compiled closure copied to board, NixOS switches configuration
4. cage restarts with new mesh-player
5. Debug: `ssh mesh@<ip> journalctl -u cage-tty1 -f`

### Native Build on Board (faster iteration)

```bash
ssh mesh@<board-ip>
cd /tmp/mesh  # clone or scp source
cargo build --release -p mesh-player  # ~30-60s incremental on RK3588S

sudo systemctl stop cage-tty1
WGPU_BACKEND=gl ./target/release/mesh-player
```

## Debugging Reference

### Audio

```bash
aplay -l                                    # list sound cards (expect 2)
speaker-test -D mesh_master -c 2 -t wav     # test master output
speaker-test -D mesh_cue -c 2 -t wav        # test cue output
pw-cli list-objects | grep -A2 "alsa_output" # PipeWire sinks
wpctl status                                 # WirePlumber routing
amixer -c rockchipes8388 set Headphone 90%   # adjust cue volume
amixer -c rockchipes8388 contents            # show all controls
```

### Display

```bash
wlr-randr                                   # connected displays
systemctl status cage-tty1                   # kiosk service status
journalctl -u cage-tty1 --no-pager -n 50     # kiosk logs
```

### Device Tree

```bash
ls /proc/device-tree/ | grep pcm5102a       # verify overlay loaded
cat /proc/device-tree/i2s@fe4a0000/status    # I2S3 status (should be "okay")
dmesg | grep -i "i2s\|pcm5102\|simple-audio" # kernel audio messages
```

### System

```bash
journalctl -u cage-tty1 -f                  # live mesh-player logs
htop                                         # CPU/memory
lsusb                                        # USB devices
cat /sys/class/thermal/thermal_zone0/temp    # temperature (divide by 1000)
ip addr                                      # network
```

### Emergency Recovery

```bash
# SSH backdoor (always up, even if cage crashes):
ssh mesh@<board-ip>
sudo systemctl restart cage-tty1

# Roll back remotely:
sudo nixos-rebuild switch --rollback

# If board won't boot: re-flash microSD with known-good image
# Or select previous NixOS generation at boot menu
```

## Further Reading

- [ARM64 Embedded Research](arm64-embedded-research.md) — Full hardware research including SBC comparison, I2S DAC technical details, display research, and MIPI DSI analysis
- [gnull/nixos-rk3588](https://github.com/gnull/nixos-rk3588) — NixOS flake for RK3588/RK3588S boards
- [tlan16/nixos-orange-5-pro](https://github.com/tlan16/nixos-orange-5-pro) — NixOS packages for Orange Pi 5 Pro
- [NixOS Wiki — Orange Pi 5](https://wiki.nixos.org/wiki/NixOS_on_ARM/Orange_Pi_5)
- [Armbian Forum — OPi 5 Max I2S](https://forum.armbian.com/topic/51422-orange-pi-5-max-enabling-i2s-for-pcm/)
- [ubuntu-rockchip I2S Discussion](https://github.com/Joshua-Riek/ubuntu-rockchip/discussions/1116)
