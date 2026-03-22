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
  boot.kernel.sysctl = {
    "kernel.printk" = "0 0 0 0";
    "vm.swappiness" = 1;                          # Keep audio buffers in RAM (was 10)
    "vm.vfs_cache_pressure" = 50;                  # Keep dentry/inode caches for music library
    "vm.dirty_ratio" = 5;                          # Limit write-back storms (read-heavy workload)
    "vm.dirty_background_ratio" = 2;
    "kernel.sched_rt_runtime_us" = -1;             # Allow RT threads 100% CPU (no 95% throttle)
    "kernel.sched_latency_ns" = 4000000;           # 4ms CFS latency (from 6ms default)
    "kernel.sched_min_granularity_ns" = 500000;    # 0.5ms CFS granularity (from 0.75ms)
    "kernel.sched_wakeup_granularity_ns" = 500000; # 0.5ms wakeup preemption (from ~1ms)
  };
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
    # RT audio optimizations: isolate A55 cores (0-3) from kernel overhead
    "transparent_hugepage=never"   # Disable THP compaction (latency spikes)
    "irqaffinity=4-7"             # Default IRQs to A76 cores (off audio cluster)
    "rcu_nocbs=0-3"               # Offload RCU callbacks from A55 audio cores
    "rcu_nocb_poll"                # Kthread polls for RCU (no IPI wakeup)
    "nohz_full=0-3"               # Tickless on A55 (no scheduler tick when single task)
    "skew_tick=1"                  # Offset timer ticks across cores (reduce lock contention)
    "nosoftlockup"                 # Disable soft lockup detector (avoids jitter)
    "nowatchdog"                   # Disable watchdog timer on audio cores
  ];

  # Disable irqbalance — conflicts with manual IRQ pinning (see audio.nix)
  services.irqbalance.enable = false;

  # Pin system services to A76 cores (keep off A55 audio cluster)
  systemd.services.NetworkManager.serviceConfig.CPUAffinity = "4-7";
  systemd.services.systemd-journald.serviceConfig.CPUAffinity = "4-7";

  # Disable deep CPU idle states on A55 audio cores (0-3)
  # C-state wakeup can take 200µs-2ms, stealing from the 5.33ms audio budget
  systemd.services.mesh-cpu-idle = {
    description = "Disable deep CPU idle states on A55 audio cores";
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = pkgs.writeShellScript "disable-cpu-idle" ''
        for cpu in 0 1 2 3; do
          for state in /sys/devices/system/cpu/cpu$cpu/cpuidle/state*/disable; do
            echo 1 > "$state" 2>/dev/null
          done
        done
        echo "Disabled deep idle states on CPUs 0-3 (A55 audio cluster)"
      '';
    };
  };

  # Don't wait for network or udev settle during boot
  systemd.services.systemd-udev-settle.enable = false;
  systemd.services.NetworkManager-wait-online.enable = false;

  # USB stick automounting (DJ plugs in USB stick with tracks)
  # udisks2 for manual `udisksctl` debugging; udev rules for automatic mount.
  # mesh-player polls /proc/mounts via sysinfo every 2s and detects new
  # removable disks under /media/. No D-Bus session or polkit needed.
  services.udisks2.enable = true;

  services.udev.extraRules = ''
    # /dev/cpu_dma_latency: allow audio group to prevent CPU idle transitions
    # mesh-player writes 0 to this fd to disable C-state transitions during playback
    KERNEL=="cpu_dma_latency", MODE="0660", GROUP="audio"

    # USB storage block device tuning for sequential audio file reads
    # - none scheduler: zero overhead (USB bus is serialized, no seek penalty)
    # - 16 MB read-ahead: keeps the USB pipe full for 130 MB FLAC files
    # - disable autosuspend: avoids first-access latency spike after idle
    ACTION=="add|change", KERNEL=="sd[a-z]", SUBSYSTEM=="block", \
      ATTR{queue/scheduler}="none", \
      ATTR{queue/read_ahead_kb}="16384"
    ACTION=="add", SUBSYSTEM=="usb", ATTR{bInterfaceClass}=="08", \
      TEST=="power/autosuspend_delay_ms", ATTR{power/autosuspend_delay_ms}="-1"

    # Auto-mount USB storage to /media/<label> (or /media/<devname> if unlabeled)
    # FAT/exFAT/NTFS lack Unix ownership — mount as mesh user so recording works.
    SUBSYSTEMS=="usb", SUBSYSTEM=="block", ACTION=="add", ENV{ID_FS_USAGE}=="filesystem", \
      RUN+="${pkgs.writeShellScript "usb-automount" ''
        LABEL="''${ID_FS_LABEL:-''${DEVNAME##*/}}"
        OPTS="noatime,X-mount.mkdir"
        case "''${ID_FS_TYPE}" in
          vfat|exfat|ntfs) OPTS="$OPTS,uid=1000,gid=100" ;;
        esac
        ${pkgs.systemd}/bin/systemd-mount --no-block --collect \
          --options="$OPTS" "$DEVNAME" "/media/$LABEL"
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
