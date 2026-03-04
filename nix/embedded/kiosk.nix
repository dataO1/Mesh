# Kiosk mode: cage Wayland compositor running mesh-player fullscreen
#
# cage is a single-application Wayland compositor (kiosk mode).
# mesh-player launches fullscreen on boot and restarts on crash.
# SSH is always available as a backdoor for debugging.
{ pkgs, lib, config, ... }:

let
  meshPlayer = config.services.mesh-embedded.package;

  # Wrapper script that sets LD_LIBRARY_PATH before exec'ing mesh-player.
  # winit/wgpu load wayland, xkbcommon, and GL libraries via dlopen() which
  # fails on NixOS without explicit library paths. We can't rely on the
  # systemd Environment= directive because PipeWire's PAM session setup
  # overrides LD_LIBRARY_PATH with just pipewire-jack/lib.
  # Helper: wait for a PipeWire node to appear, then link ports.
  # PipeWire JACK clients with node.always-process=true stick on Dummy-Driver
  # unless explicit links are created to a real ALSA sink. We start mesh-player
  # in the background, wait for its ports, and then pw-link them to ES8388.
  meshPlayerWrapper = pkgs.writeShellScript "mesh-player-wrapper" ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
      pkgs.wayland
      pkgs.libxkbcommon
      pkgs.libGL
      pkgs.vulkan-loader
    ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export RUST_LOG="''${RUST_LOG:-info}"

    PW_LINK="${pkgs.pipewire}/bin/pw-link"
    PW_CLI="${pkgs.pipewire}/bin/pw-cli"
    SYSTEMD_CAT="${pkgs.systemd}/bin/systemd-cat"
    ES8388="alsa_output.platform-es8388-sound.stereo-fallback"

    # Set up logging pipe — process substitution doesn't survive pw-jack's exec
    LOGFIFO=$(mktemp -u /tmp/mesh-log.XXXXXX)
    mkfifo "$LOGFIFO"
    $SYSTEMD_CAT -t mesh-player < "$LOGFIFO" &
    LOGCAT_PID=$!

    # Wait for PipeWire to enumerate the ES8388 ALSA node (up to 15s)
    for i in $(seq 1 30); do
      $PW_CLI list-objects Node 2>/dev/null | grep -q es8388 && break
      sleep 0.5
    done

    # Start mesh-player via pw-jack in background, log to journald via FIFO
    ${pkgs.pipewire.jack}/bin/pw-jack ${meshPlayer}/bin/mesh-player "$@" \
      >"$LOGFIFO" 2>&1 &
    PLAYER_PID=$!

    # Wait for mesh-player's JACK ports to appear (up to 10s)
    for i in $(seq 1 50); do
      $PW_LINK -o 2>/dev/null | grep -q "mesh-player:master_left" && break
      sleep 0.2
    done

    # Link master output to ES8388 headphone jack
    $PW_LINK "mesh-player:master_left"  "$ES8388:playback_FL" 2>/dev/null
    $PW_LINK "mesh-player:master_right" "$ES8388:playback_FR" 2>/dev/null

    # Wait for mesh-player to exit (cage restarts on crash)
    wait $PLAYER_PID
    kill $LOGCAT_PID 2>/dev/null
    rm -f "$LOGFIFO"
  '';
in
{
  options.services.mesh-embedded = {
    package = lib.mkOption {
      type = lib.types.package;
      description = "The mesh-player package to run in kiosk mode";
    };
  };

  config = {
    # Mesh user account (wheel for sudo access)
    users.users.mesh = {
      isNormalUser = true;
      extraGroups = [ "audio" "video" "input" "plugdev" "wheel" "networkmanager" ];
      initialPassword = "mesh";
    };

    # Passwordless sudo for wheel group (mesh user)
    security.sudo.wheelNeedsPassword = false;

    # cage Wayland kiosk compositor
    services.cage = {
      enable = true;
      user = "mesh";
      program = "${meshPlayerWrapper}";
      # -d: use DRM backend directly (no libseat)
      # -s: allow VT switching (Ctrl+Alt+F2 for debug tty)
      # -m last: single output only — "extend" (default) creates a combined
      #   surface (e.g. 3840x1080) that exceeds Mali G610's max texture dim (2048)
      extraArguments = [ "-d" "-s" "-m" "last" ];
      environment = {
        # Vulkan via PanVK (Mali-G610, conformant Vulkan 1.2+)
        # Mailbox: low-latency tearless presentation (1-frame queue vs Fifo's 3)
        WGPU_BACKEND = "vulkan";
        ICED_PRESENT_MODE = "mailbox";
        WLR_NO_HARDWARE_CURSORS = "1";
      };
    };

    # Crash restart + RT limits + all cores.
    # App manages per-thread affinity internally (audio → A55, loaders → A76).
    #
    # CRITICAL: PAM loginLimits do NOT apply to systemd services.
    # cage-tty1 inherits limits from its unit, not from /etc/security/limits.conf.
    # Without these, RLIMIT_RTPRIO=0 and sched_setscheduler(SCHED_FIFO) fails,
    # making CPU pinning counterproductive (pinned threads can't preempt).
    systemd.services."cage-tty1" = {
      serviceConfig = {
        Restart = "always";
        RestartSec = 2;
        CPUAffinity = "0-7";
        # RT scheduling: allow SCHED_FIFO up to priority 95
        # (rayon audio workers use 70, PipeWire data thread uses 88)
        LimitRTPRIO = "95";
        # Memory locking: allow mlockall() to pin all pages in RAM
        # (prevents page faults during audio processing)
        LimitMEMLOCK = "infinity";
        # Nice level: allow high-priority scheduling
        LimitNICE = "-20";
      };
    };

    # Seed default config files on first boot (C = copy-if-not-exists)
    systemd.tmpfiles.rules = [
      "d /home/mesh/Music 0755 mesh mesh -"
      "d /home/mesh/Music/mesh-collection 0755 mesh mesh -"
      "C /home/mesh/Music/mesh-collection/midi.yaml 0644 mesh mesh - ${../../config/midi.yaml}"
      "C /home/mesh/Music/mesh-collection/slicer-presets.yaml 0644 mesh mesh - ${../../config/slicer-presets.yaml}"
      "C /home/mesh/Music/mesh-collection/theme.yaml 0644 mesh mesh - ${../../config/theme.yaml}"
    ];

    # TTY2 login shell for local debugging (Ctrl+Alt+F2 when cage has -s flag)
    systemd.services."getty@tty2".enable = true;
    systemd.services."getty@tty2".wantedBy = [ "multi-user.target" ];

    # Persistent journal (survives reboots — critical for diagnosing crash loops)
    services.journald.extraConfig = ''
      Storage=persistent
      SystemMaxUse=50M
    '';

    # SSH backdoor (always available, even if cage crashes)
    services.openssh = {
      enable = true;
      settings.PasswordAuthentication = true;
    };
    networking.firewall.allowedTCPPorts = [ 22 ];

    # Allow mesh user to trigger updates via systemd service
    systemd.services.mesh-update = {
      description = "Mesh Player System Update";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = pkgs.writeShellScript "mesh-update" ''
          set -euo pipefail
          VERSION=$(cat /var/lib/mesh/update-target 2>/dev/null || echo "")
          if [ -z "$VERSION" ]; then
            echo "No update target set"
            exit 1
          fi
          echo "Updating to $VERSION..."
          ${config.system.build.nixos-rebuild}/bin/nixos-rebuild switch \
            --flake "github:dataO1/Mesh/$VERSION#mesh-embedded" \
            --no-write-lock-file
          rm -f /var/lib/mesh/update-target
        '';
      };
    };

    # Polkit rule: allow mesh user to manage update/cage services and power off
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            (action.lookup("unit") == "mesh-update.service" ||
             action.lookup("unit") == "cage-tty1.service") &&
            subject.user == "mesh") {
          return polkit.Result.YES;
        }
        if (action.id == "org.freedesktop.login1.power-off" &&
            subject.user == "mesh") {
          return polkit.Result.YES;
        }
      });
    '';
  };
}
