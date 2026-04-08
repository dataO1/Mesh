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

    WLR_RANDR="${pkgs.wlr-randr}/bin/wlr-randr"
    PW_LINK="${pkgs.pipewire}/bin/pw-link"
    PW_CLI="${pkgs.pipewire}/bin/pw-cli"
    SYSTEMD_CAT="${pkgs.systemd}/bin/systemd-cat"
    ES8388="alsa_output.platform-es8388-sound.stereo-fallback"

    # Switch to highest refresh rate for the active mode's resolution
    # (EDID preferred mode is often 60Hz; the panel supports 120Hz)
    $WLR_RANDR --output HDMI-A-1 --mode 2880x864@120.002998Hz 2>/dev/null || true

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
        # GL via Panfrost (Mali-G610) — Vulkan (PanVK) crashes on non-standard
        # resolutions like 2880x864. Panfrost GL is mature and handles all modes.
        # Note: Mailbox present mode is Vulkan-only; GL uses Fifo (vsync).
        WGPU_BACKEND = "gl";
        ICED_PRESENT_MODE = "fifo";
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
      # Do NOT let nixos-rebuild switch restart cage automatically.
      #
      # switch-to-configuration detects that cage-tty1's ExecStart changed
      # (because ${meshPlayer} is a new store path) and issues a restart
      # during the activation phase — while other services are also being
      # reconfigured. That 2-second RestartSec window during activation is
      # where autovt@tty1 races in and claims tty1 first.
      #
      # mesh-update.service handles the restart explicitly via ExecStartPost,
      # after nixos-rebuild has fully settled. That restart is clean.
      restartIfChanged = false;

      # Belt-and-suspenders: conflict with both the explicit getty unit AND
      # the autovt template (logind's on-demand VT activation). The NixOS
      # cage module already adds Conflicts=getty@tty1, but autovt@tty1 is
      # a separate unit that may not be covered by that declaration.
      conflicts = [ "getty@tty1.service" "autovt@tty1.service" ];

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
        # Nice level: allow nice -20 (with - prefix = nice value, not raw rlimit)
        LimitNICE = "-20";
        # I/O scheduling: realtime class ensures track file reads aren't
        # starved by USB I/O, journald, or other background disk activity
        IOSchedulingClass = "realtime";
        IOSchedulingPriority = 0;
        # OOM protection: prevent the OOM killer from targeting the audio process
        OOMScoreAdjust = -1000;
      };
    };

    # Seed default config files on first boot (C = copy-if-not-exists)
    systemd.tmpfiles.rules = [
      "d /home/mesh/Music 0755 mesh users -"
      "d /home/mesh/Music/mesh-collection 0755 mesh users -"
      "C /home/mesh/Music/mesh-collection/midi.yaml 0644 mesh users - ${../../config/midi.yaml}"
      "C /home/mesh/Music/mesh-collection/slicer-presets.yaml 0644 mesh users - ${../../config/slicer-presets.yaml}"
    ];

    # Theme file is force-updated on every activation (OTA update) so new
    # default themes reach the device. Unlike midi/slicer configs, theme
    # definitions are managed upstream and should track the release.
    system.activationScripts.meshTheme.text = ''
      install -D -m 0644 -o mesh -g users ${../../config/theme.yaml} /home/mesh/Music/mesh-collection/theme.yaml
    '';

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
            --no-write-lock-file \
            --refresh
          rm -f /var/lib/mesh/update-target
        '';
        # Restart cage after the update. cage-tty1 has restartIfChanged=false so
        # nixos-rebuild's activation script won't restart it — we do it here
        # instead, after activation has fully settled and no other services are
        # being reconfigured. This eliminates the autovt@tty1 race window.
        # The - prefix means a non-zero exit code is ignored (cage may already
        # be stopped if the update itself crashed mesh-player).
        ExecStartPost = "-${pkgs.systemd}/bin/systemctl restart cage-tty1.service";
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
