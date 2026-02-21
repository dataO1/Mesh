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
  meshPlayerWrapper = pkgs.writeShellScript "mesh-player-wrapper" ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
      pkgs.wayland
      pkgs.libxkbcommon
      pkgs.libGL
      pkgs.vulkan-loader
    ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    exec ${pkgs.pipewire}/bin/pw-jack ${meshPlayer}/bin/mesh-player "$@"
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
