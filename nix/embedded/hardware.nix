# Orange Pi 5 Pro hardware configuration
#
# Board-specific: RK3588S SoC, Mali-G610 GPU, PCM5102A I2S DAC on GPIO.
# The nixos-rk3588 module (imported in flake.nix) provides kernel and
# base board support. This module adds mesh-specific hardware config.
{ pkgs, lib, ... }:

{
  # PCM5102A I2S DAC on GPIO header — registers as ALSA card "PCM5102A"
  hardware.deviceTree.overlays = [
    { name = "pcm5102a-i2s3"; dtsFile = ./pcm5102a-i2s3.dts; }
  ];

  # GPU: Mali-G610 via PanVK (Vulkan) or Panfrost (GLES)
  hardware.graphics.enable = true;

  # Performance: pin big cores to max frequency for real-time audio
  powerManagement.cpuFreqGovernor = "performance";

  # Fast boot: skip unnecessary delays
  boot.loader.timeout = 0;
  boot.plymouth.enable = false;
  boot.initrd.systemd.enable = true;

  # Don't wait for network or udev settle during boot
  systemd.services.systemd-udev-settle.enable = false;
  systemd.services.NetworkManager-wait-online.enable = false;

  # USB stick automounting (DJ plugs in USB stick with tracks)
  services.udisks2.enable = true;
}
