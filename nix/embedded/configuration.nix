# Base NixOS configuration for the mesh embedded player
#
# This is the top-level NixOS module that imports all embedded subsystems.
# Board-specific support (kernel, bootloader) comes from nixos-rk3588
# which is imported in flake.nix.
{ pkgs, lib, ... }:

{
  imports = [
    ./hardware.nix
    ./audio.nix
    ./kiosk.nix
  ];

  system.stateVersion = "24.11";
  networking.hostName = "mesh-embedded";
  time.timeZone = "UTC";

  # WiFi (built-in on OPi 5)
  networking.networkmanager.enable = true;

  # Nix configuration
  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "mesh" ];

    # Binary caches: custom mesh cache (GitHub Pages) + official NixOS cache
    # The mesh cache hosts pre-built aarch64 packages (mesh-player, essentia, etc.)
    # Standard NixOS packages come from cache.nixos.org
    substituters = [
      "https://datao1.github.io/Mesh/"
      "https://cache.nixos.org/"
    ];
    trusted-public-keys = [
      # Replace with output of: cat cache-pub-key.pem
      # Generated via: nix-store --generate-binary-cache-key mesh-embedded cache-priv-key.pem cache-pub-key.pem
      "mesh-embedded:TVnMdLIfPt4q20ulKuieSc2Rv2fcwnph/TdLh2dZuKA="
      "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
    ];
  };

  # System packages for debugging and maintenance
  environment.systemPackages = with pkgs; [
    vim
    htop
    alsa-utils
    usbutils
    pciutils
    dtc
    wlr-randr
    evtest
  ];

  # Persistent state directory for mesh-player
  systemd.tmpfiles.rules = [
    "d /var/lib/mesh 0755 mesh mesh -"
  ];
}
