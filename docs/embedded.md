# Embedded Standalone Setup

## Overview

Mesh can run standalone on an Orange Pi 5 Pro single-board computer -- a full DJ system for roughly $112 in hardware, no laptop required. The device boots directly into mesh-player in fullscreen kiosk mode (NixOS + cage Wayland compositor). Load tracks from USB 3.0 sticks, connect MIDI controllers, and perform.

## Hardware

### Bill of Materials

| Component | Description | Approx. Cost |
|-----------|-------------|:------------:|
| Orange Pi 5 Pro 8GB | RK3588S SoC (4x A76 + 4x A55), WiFi 5, USB 3.0, HDMI | ~$80 |
| PCM5102A I2S DAC board | Master audio output via GPIO header. 112 dB SNR | ~$5 |
| ES8388 onboard codec | Headphone cue output via 3.5mm TRRS (built into OPi5) | Included |
| microSD card (32GB+) | NixOS system image. UHS-I or faster recommended | ~$8 |
| USB 3.0 stick | Your music collection (exported from mesh-cue) | ~$10-20 |
| 5V/4A USB-C power supply | Must provide stable 4A -- underpowered PSUs cause throttling | ~$10 |
| Enclosure (optional) | 3D printed or commercial Orange Pi 5 case | ~$5-15 |
| HDMI display or touchscreen | Any HDMI monitor. 7" touchscreen works for compact setups | ~$30-80 |

Total core BOM (without display and USB): ~$112

### Audio Architecture

Two separate audio outputs:

- **Master (speakers/PA):** PCM5102A I2S DAC on GPIO header. High-quality output for PA.
- **Cue (headphones):** ES8388 onboard codec via 3.5mm TRRS. Adequate for monitoring.

Both run through PipeWire with JACK bridge at 48 kHz / 256 samples (~5.3ms latency).

### Wiring the PCM5102A DAC

| DAC Pin | GPIO Pin | Function |
|---------|----------|----------|
| VIN | Pin 4 (5V) | Power |
| GND | Pin 6 (GND) | Ground |
| BCK | Pin 12 (I2S BCLK) | Bit clock |
| LCK | Pin 35 (I2S LRCK) | Word clock |
| DIN | Pin 40 (I2S SDATA) | Audio data |

I2S is enabled via device tree overlay in the NixOS config. No soldering -- jumper wires work.

<!-- TODO: Photo -- PCM5102A wiring to Orange Pi GPIO header with labeled connections -->

## Installation

### Step 1: Download the SD Image

Find the latest `sdimage-*` release on [GitHub Releases](https://github.com/dataO1/Mesh/releases). The image is compressed with zstd.

### Step 2: Flash to microSD

```bash
# Linux
zstdcat nixos-sd-image-*.img.zst | sudo dd of=/dev/sdX bs=4M status=progress

# macOS (use rdisk for speed)
zstdcat nixos-sd-image-*.img.zst | sudo dd of=/dev/rdiskN bs=4m

# Windows: Use Etcher or Rufus -- decompress the .zst first with a tool like 7-Zip
```

Replace `/dev/sdX` with your SD card device. Double-check you have the right device.

### Step 3: First Boot

1. Insert the microSD card
2. Connect HDMI display
3. Connect USB-C power (5V/4A)
4. Connect USB 3.0 stick with your mesh collection (exported from mesh-cue)
5. Wait ~30 seconds for boot
6. mesh-player starts automatically in fullscreen

### Step 4: Connect Controllers

Plug MIDI or HID controllers via USB. If no MIDI mapping exists, mesh-player enters the MIDI Learn wizard automatically.

## Network Setup

### WiFi

Configure via Settings > Network in mesh-player:

1. Scan for available networks
2. Select a network and enter the password (on-screen keyboard)
3. Connection persists across reboots (NetworkManager)

### Ethernet

Plug in an Ethernet cable -- auto-connects via DHCP.

Network is only needed for over-the-air updates. Mesh does not require internet for any DJ functionality.

## Over-the-Air Updates

1. Open Settings > System Update
2. Click "Check for Update" -- queries GitHub for the latest release
3. If available, click "Install Update"
4. The device downloads pre-built packages from the binary cache (never compiles on-device)
5. Progress shown from systemd journal
6. Click "Restart" when complete

Toggle "Pre-release Updates" in Settings to include RC/beta versions.

Updates typically take 2-5 minutes on a decent WiFi connection.

## Performance

### Real-Time Optimizations

The `embedded-rt` build includes:

- **CPU affinity:** audio threads pinned to fast A76 cores (4-7), background tasks on A55 cores (0-3)
- **Audio IRQ pinning:** interrupt handlers on a dedicated A55 core
- **Deep idle disabled:** C-state wakeup latency eliminated on audio cores
- **Kernel tuning:** tickless operation, RCU offloading, 4ms CFS scheduler latency
- **Memory locking:** audio buffers locked in RAM

### Expected Performance

| Metric | Value |
|--------|-------|
| Audio latency | ~5.3ms (256 samples @ 48 kHz) |
| Track loading | 2-4 seconds from USB 3.0 |
| Boot time | ~25-35 seconds to fullscreen |
| CPU usage (4 decks) | ~40-60% total |
| RAM usage | ~1.5 GB typical |
| GPU | Mali-G610 via PanVK (Vulkan) |

## Limitations

- **No mesh-cue:** Only mesh-player runs on embedded. Prepare tracks on a desktop/laptop first.
- **No local collection:** All tracks loaded from USB sticks. SD card is for the OS only.
- **USB 2.0 is slower:** Use the USB 3.0 port (blue) for music. USB 2.0 works but loading takes longer.
- **Heavy CLAP plugins:** Some CPU-intensive plugins may cause frame drops. Lighter plugins and native effects work well.
- **No stem separation:** Cannot run Demucs on-device. Prepare tracks with mesh-cue first.
- **WiFi range:** Onboard antenna has limited range. Stay within reasonable distance for OTA updates.

## Troubleshooting

### No audio output

- Check DAC wiring against the pin table above
- Verify 5V power to DAC (LED should be lit on most PCM5102A boards)
- In Settings > Audio Output, ensure the I2S device is selected for master

### No display

- Ensure HDMI is connected before powering on
- Try a different HDMI cable (some have compatibility issues with RK3588)
- Check power supply -- insufficient power causes display init failure

### USB stick not detected

- Use the USB 3.0 port (blue connector)
- Ensure the stick has a `mesh-collection/` folder (exported from mesh-cue)
- Try exFAT format if FAT32 causes issues with large files

### Audio dropouts

- Check CPU temperature (throttling at 85C). Ensure adequate ventilation or a heatsink.
- Reduce the number of active effects
- Disable PD effects if CPU-limited

### Update fails

- Verify WiFi connection in Settings > Network
- Retry -- transient network errors can cause download failures

## Advanced: Custom Builds

Build the SD image yourself (requires Linux with Nix):

```bash
nix build .#nixosConfigurations.mesh-embedded.config.system.build.sdImage
zstdcat result/sd-image/*.img.zst | sudo dd of=/dev/sdX bs=4M status=progress
```

The build uses the binary cache when available, or compiles natively on ARM.
