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

    PW_LINK="${pkgs.pipewire}/bin/pw-link"
    PW_CLI="${pkgs.pipewire}/bin/pw-cli"
    ES8388="alsa_output.platform-es8388-sound.stereo-fallback"

    # Wait for PipeWire to enumerate the ES8388 ALSA node (up to 15s)
    for i in $(seq 1 30); do
      $PW_CLI list-objects Node 2>/dev/null | grep -q es8388 && break
      sleep 0.5
    done

    # Start mesh-player via pw-jack in background, log to journald
    ${pkgs.pipewire.jack}/bin/pw-jack ${meshPlayer}/bin/mesh-player "$@" \
      > >(${pkgs.systemd}/bin/systemd-cat -t mesh-player) 2>&1 &
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
      extraArguments = [ "-d" "-s" ];
      environment = {
        # Use GLES via Panthor (Mali-G610)
        WGPU_BACKEND = "gl";
        MESA_GL_VERSION_OVERRIDE = "3.1";
        WLR_NO_HARDWARE_CURSORS = "1";
      };
    };

    # Crash restart + pin to A76 big cores (cores 4-7)
    systemd.services."cage-tty1" = {
      serviceConfig = {
        Restart = "always";
        RestartSec = 2;
        CPUAffinity = "4-7";
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

    # Polkit rule: allow mesh user to manage update and cage services
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            (action.lookup("unit") == "mesh-update.service" ||
             action.lookup("unit") == "cage-tty1.service") &&
            subject.user == "mesh") {
          return polkit.Result.YES;
        }
      });
    '';
  };
}
