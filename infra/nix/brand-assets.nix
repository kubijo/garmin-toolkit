{ pkgs, source }:

pkgs.runCommandLocal "garmin-toolkit-brand-assets"
  {
    nativeBuildInputs = [
      pkgs.imagemagick
      pkgs.resvg
    ];
  }
  ''
    install -Dm444 ${source} "$out/icon.svg"

    resvg -w 128 -h 128 ${source} "$out/icon-128.png"
    resvg -w 256 -h 256 ${source} "$out/icon-256.png"
    resvg -w 512 -h 512 ${source} "$out/icon-512.png"

    magick "$out/icon-128.png" -resize 100x100 mark.png
    magick -size 250x100 canvas:none mark.png -gravity center -composite "$out/logo-250x100.png"
  ''
