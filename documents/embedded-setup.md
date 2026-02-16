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

Linux needs a Device Tree Overlay to enable the I2S3 controller and register the DAC as a sound card.

Save as `pcm5102a-i2s3.dts`:

```dts
/dts-v1/;
/plugin/;

/ {
    compatible = "xunlong,orangepi-5-plus", "rockchip,rk3588";

    fragment@0 {
        target = <&i2s3_2ch>;
        __overlay__ {
            status = "okay";
            #sound-dai-cells = <0>;
            pinctrl-names = "default";
            pinctrl-0 = <&i2s3m0_sclk &i2s3m0_lrck &i2s3m0_sdo>;
            rockchip,playback-channels = <2>;
        };
    };

    fragment@1 {
        target-path = "/";
        __overlay__ {
            pcm5102a_codec: pcm5102a {
                compatible = "ti,pcm5102a";
                #sound-dai-cells = <0>;
            };

            pcm5102a_sound: pcm5102a-sound {
                compatible = "simple-audio-card";
                simple-audio-card,name = "PCM5102A";
                simple-audio-card,format = "i2s";

                simple-audio-card,cpu {
                    sound-dai = <&i2s3_2ch>;
                };
                simple-audio-card,codec {
                    sound-dai = <&pcm5102a_codec>;
                };
            };
        };
    };
};
```

Compile and install (non-NixOS):

```bash
dtc -I dts -O dtb -o pcm5102a-i2s3.dtbo pcm5102a-i2s3.dts
sudo cp pcm5102a-i2s3.dtbo /boot/dtb/rockchip/overlay/
sudo orangepi-config  # System → Hardware → enable overlay
```

On NixOS, the overlay is loaded automatically via `hardware.deviceTree.overlays` (see below).

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

```rust
// Device selection pseudocode
let host = cpal::default_host();
let devices = host.output_devices()?;

let master_device = devices.clone()
    .find(|d| d.name().unwrap_or_default().contains("PCM5102A"))?;
let cue_device = devices
    .find(|d| d.name().unwrap_or_default().contains("es8388"))?;

let master_stream = master_device.build_output_stream(&config, master_callback, err_fn, None)?;
let cue_stream = cue_device.build_output_stream(&config, cue_callback, err_fn, None)?;
```

Named ALSA aliases provide stable device names regardless of card numbering:

```
# /etc/alsa/conf.d/99-mesh.conf
pcm.mesh_master {
  type hw
  card "PCM5102A"
  device 0
}
pcm.mesh_cue {
  type hw
  card "rockchipes8388"
  device 0
}
```

### Option B: PipeWire routing (for development/debugging)

PipeWire exposes both cards as sinks with runtime re-routing via `pavucontrol` or `pw-link`. Adds ~2-5ms latency.

## NixOS Setup

### Prerequisites

On your x86 NixOS workstation, enable aarch64 emulation:

```nix
# In your workstation's configuration.nix:
boot.binfmt.emulatedSystems = [ "aarch64-linux" ];
```

Then `sudo nixos-rebuild switch` and reboot. This lets your x86 machine build aarch64 packages using QEMU — 95% of packages come pre-built from `cache.nixos.org`, only mesh-player needs actual compilation.

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

### Step 2: Build NixOS Image

```bash
cd ~/Projects/mesh
nix build .#nixosConfigurations.mesh-embedded.config.system.build.sdImage

# Flash to microSD
dd if=result/sd-image/*.img of=/dev/sdX bs=1M status=progress
```

### Step 3: First Boot

Insert microSD, connect HDMI + USB keyboard, power on. NixOS boots to login. SSH is enabled. All future updates happen remotely.

```bash
ssh mesh@<board-ip>
```

### Flake Structure

```
mesh/
├── flake.nix                    # mesh-embedded NixOS config
├── nix/
│   ├── common.nix               # Existing build deps
│   └── embedded/
│       ├── configuration.nix    # NixOS system config
│       ├── hardware.nix         # OPi 5 Pro hardware (kernel, DT, GPU)
│       ├── audio.nix            # PipeWire + I2S DAC overlay + ALSA config
│       ├── kiosk.nix            # cage compositor + auto-login
│       └── pcm5102a-i2s3.dts   # Device tree overlay source
```

### NixOS Modules

**flake.nix:**

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixos-rk3588.url = "github:gnull/nixos-rk3588";
  };

  outputs = { self, nixpkgs, nixos-rk3588, ... }: {
    nixosConfigurations.mesh-embedded = nixpkgs.lib.nixosSystem {
      system = "aarch64-linux";
      modules = [
        nixos-rk3588.nixosModules.orangepi5
        ./nix/embedded/configuration.nix
        ./nix/embedded/hardware.nix
        ./nix/embedded/audio.nix
        ./nix/embedded/kiosk.nix
      ];
    };
  };
}
```

**hardware.nix:**

```nix
{ pkgs, ... }: {
  hardware.graphics.enable = true;

  hardware.deviceTree.overlays = [
    { name = "pcm5102a-i2s3"; dtsFile = ./pcm5102a-i2s3.dts; }
  ];

  powerManagement.cpuFreqGovernor = "performance";

  boot.loader.timeout = 0;
  boot.plymouth.enable = false;
  boot.initrd.systemd.enable = true;
  systemd.services.systemd-udev-settle.enable = false;
  systemd.services.NetworkManager-wait-online.enable = false;

  services.udisks2.enable = true;  # USB stick automounting
}
```

**audio.nix:**

```nix
{ pkgs, ... }: {
  security.rtkit.enable = true;
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = false;
    jack.enable = true;
  };

  hardware.alsa.enablePersistence = true;

  environment.etc."alsa/conf.d/99-mesh.conf".text = ''
    pcm.mesh_master {
      type hw
      card "PCM5102A"
      device 0
    }
    pcm.mesh_cue {
      type hw
      card "rockchipes8388"
      device 0
    }
  '';

  services.pipewire.wireplumber.extraConfig."99-mesh-audio" = {
    "monitor.alsa.rules" = [
      {
        matches = [
          { "node.name" = "~alsa_output.*es8388*"; }
          { "node.name" = "~alsa_output.*PCM5102A*"; }
        ];
        actions = {
          update-props = {
            "session.suspend-timeout-seconds" = 0;
            "api.alsa.period-size" = 256;
            "api.alsa.headroom" = 256;
          };
        };
      }
    ];
  };

  systemd.services.mesh-audio-init = {
    description = "Initialize audio card volumes";
    after = [ "sound.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${pkgs.alsa-utils}/bin/amixer -c rockchipes8388 set Headphone 80%";
    };
  };
}
```

**kiosk.nix:**

```nix
{ pkgs, ... }:
let
  meshPlayer = pkgs.callPackage ../../default.nix {};
in {
  users.users.mesh = {
    isNormalUser = true;
    extraGroups = [ "audio" "video" "input" "plugdev" ];
    initialPassword = "mesh";
  };

  services.cage = {
    enable = true;
    user = "mesh";
    program = "${meshPlayer}/bin/mesh-player";
    extraArguments = [ "-d" ];
    environment = {
      WGPU_BACKEND = "gl";
      MESA_GL_VERSION_OVERRIDE = "3.1";
      WLR_NO_HARDWARE_CURSORS = "1";
    };
  };

  systemd.services."cage-tty1" = {
    serviceConfig = {
      Restart = "always";
      RestartSec = 2;
      CPUAffinity = "4-7";  # pin to A76 big cores
    };
  };

  services.openssh = {
    enable = true;
    settings.PasswordAuthentication = true;
  };

  networking.firewall.allowedTCPPorts = [ 22 ];
}
```

**configuration.nix:**

```nix
{ pkgs, ... }: {
  system.stateVersion = "24.11";
  networking.hostName = "mesh-embedded";
  time.timeZone = "Europe/Berlin";
  networking.networkmanager.enable = true;

  environment.systemPackages = with pkgs; [
    vim htop alsa-utils usbutils pciutils dtc wlr-randr evtest
  ];

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "mesh" ];
  };
}
```

## Deploying Updates

### From Your Workstation (SSH)

```bash
nixos-rebuild switch \
  --flake .#mesh-embedded \
  --target-host mesh@192.168.1.100 \
  --use-remote-sudo
```

This builds the system closure locally (QEMU emulation + binary cache), copies changed store paths to the board via SSH, and activates the new configuration. mesh-player restarts automatically.

### Rollback

```bash
# If something breaks:
nixos-rebuild switch --rollback \
  --target-host mesh@192.168.1.100 \
  --use-remote-sudo
```

NixOS atomic updates mean old generations are preserved, no partial updates occur, and the board is never in a broken state — even power loss during update is safe.

### OTA Over WiFi

```bash
nixos-rebuild switch --flake .#mesh-embedded --target-host mesh@mesh-embedded.local --use-remote-sudo
```

For remote boards on different networks, add Tailscale:

```nix
services.tailscale.enable = true;
```

## Development Workflow

### Remote Deploy (standard)

```
  x86 workstation                          OPi 5 Pro
  ┌──────────────┐                         ┌──────────────────────┐
  │ Edit Rust code│                         │ NixOS running        │
  │ in mesh repo  │                         │ cage → mesh-player   │
  │               │                         │                      │
  │ nixos-rebuild │──── SSH ───────────────▶│ switch-to-config     │
  │ --target-host │     (copy store paths)  │ restart cage service │
  │               │                         │ mesh-player starts   │
  │               │◀─── SSH ────────────────│ journalctl -f output │
  │ See logs      │     (debug)             │                      │
  └──────────────┘                         └──────────────────────┘
```

1. Edit Rust code on x86 workstation
2. `nixos-rebuild switch --target-host mesh@<ip> --use-remote-sudo`
3. Build via QEMU (~2-5 min incremental)
4. New binary copied to board, cage restarts
5. Debug: `journalctl -u cage-tty1 -f`

### Native Build on Board (faster iteration)

```bash
ssh mesh@192.168.1.100
cd mesh
cargo build --release -p mesh-player  # ~30-60s incremental

sudo systemctl stop cage-tty1
WGPU_BACKEND=gl ./target/release/mesh-player
```

### Cachix (optional CI/CD)

```bash
cachix push mesh-embedded $(nix build .#mesh-player-aarch64 --print-out-paths)
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
