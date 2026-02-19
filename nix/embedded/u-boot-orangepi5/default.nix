# Prebuilt U-Boot binaries for Orange Pi 5 (RK3588S)
#
# Extracted from the official Orange Pi Debian image (v1.1.8, Feb 2024).
# These are dd'd into the SD image gap before the first partition so the
# board boots without needing U-Boot pre-flashed to SPI NOR flash.
#
# idbloader.img — Rockchip TPL + SPL (sector 64 / 0x8000)
# u-boot.itb   — U-Boot proper + ATF as FIT image (sector 16384 / 0x800000)
{ runCommand }:

runCommand "u-boot-orangepi5" { } ''
  mkdir -p $out
  cp ${./idbloader.img} $out/idbloader.img
  cp ${./u-boot.itb} $out/u-boot.itb
''
