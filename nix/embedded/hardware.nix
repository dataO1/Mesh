# Orange Pi 5 hardware configuration
#
# Board-specific: RK3588S SoC, Mali-G610 GPU, PCM5102A I2S DAC on GPIO.
# The nixos-rk3588 module (imported in flake.nix) provides kernel and
# base board support. This module adds mesh-specific hardware config.
{ pkgs, lib, ... }:

{
  # PCM5102A I2S DAC on GPIO header — registers as ALSA card "PCM5102A"
  # Filter restricts overlay application to just the Orange Pi 5 DTB
  # (without this, NixOS applies overlays to ALL kernel DTBs and fails on
  # boards that don't have the i2s3_2ch node)
  hardware.deviceTree.filter = "rk3588s-orangepi-5*.dtb";
  hardware.deviceTree.overlays = [
    { name = "pcm5102a-i2s3"; dtsFile = ./pcm5102a-i2s3.dts; }
  ];

  # GPU: Mali-G610 via PanVK (Vulkan) or Panfrost (GLES)
  hardware.graphics.enable = true;

  # Performance: pin big cores to max frequency for real-time audio
  powerManagement.cpuFreqGovernor = "performance";

  # Fast boot: skip unnecessary delays
  boot.loader.timeout = 0;
  boot.initrd.systemd.enable = true;
  boot.initrd.systemd.emergencyAccess = true;

  # Silent boot — black screen from power-on until cage/mesh-player starts
  # console=tty2 redirects ALL output off the display; cage runs on tty1
  # Debug: switch to tty2 (Ctrl+Alt+F2) or use journalctl -b
  boot.consoleLogLevel = 0;
  boot.initrd.verbose = false;
  boot.kernel.sysctl."kernel.printk" = "0 0 0 0";
  boot.kernelParams = [
    "quiet"
    "loglevel=0"
    "systemd.show_status=false"
    "rd.systemd.show_status=false"
    "udev.log_level=3"
    "rd.udev.log_level=3"
    "vt.global_cursor_default=0"
    "logo.nologo"
    "threadirqs"
  ];

  # Low swappiness: keep audio buffers in RAM
  boot.kernel.sysctl."vm.swappiness" = 10;

  # Don't wait for network or udev settle during boot
  systemd.services.systemd-udev-settle.enable = false;
  systemd.services.NetworkManager-wait-online.enable = false;

  # USB stick automounting (DJ plugs in USB stick with tracks)
  # udisks2 for manual `udisksctl` debugging; udev rules for automatic mount.
  # mesh-player polls /proc/mounts via sysinfo every 2s and detects new
  # removable disks under /media/. No D-Bus session or polkit needed.
  services.udisks2.enable = true;

  services.udev.extraRules = ''
    # Auto-mount USB storage to /media/<label> (or /media/<devname> if unlabeled)
    SUBSYSTEMS=="usb", SUBSYSTEM=="block", ACTION=="add", ENV{ID_FS_USAGE}=="filesystem", \
      RUN+="${pkgs.writeShellScript "usb-automount" ''
        LABEL="''${ID_FS_LABEL:-''${DEVNAME##*/}}"
        ${pkgs.systemd}/bin/systemd-mount --no-block --collect \
          --options=noatime,X-mount.mkdir "$DEVNAME" "/media/$LABEL"
      ''}"

    # Clean up on removal
    SUBSYSTEMS=="usb", SUBSYSTEM=="block", ACTION=="remove", ENV{ID_FS_USAGE}=="filesystem", \
      RUN+="${pkgs.writeShellScript "usb-autoumount" ''
        LABEL="''${ID_FS_LABEL:-''${DEVNAME##*/}}"
        ${pkgs.systemd}/bin/systemd-umount "/media/$LABEL" 2>/dev/null || true
        ${pkgs.coreutils}/bin/rmdir "/media/$LABEL" 2>/dev/null || true
      ''}"
  '';
}
