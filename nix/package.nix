{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  gtk4,
  libadwaita,
  openssl,
  gst_all_1,
  ffmpeg,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;

  buildType = "debug";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../migrations
      ../tests
      ../examples
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
    rustPlatform.bindgenHook
  ];

  buildInputs = [
    gtk4
    libadwaita
    openssl
  ]
  ++ (with gst_all_1; [
    gstreamer
    gst-plugins-base
    gst-plugins-good
    gst-plugins-bad
    gst-libav
    # Provides gtk4paintablesink.
    gst-plugins-rs
  ]);

  # Downloads are muxed with ffmpeg at runtime.
  preFixup = ''
    gappsWrapperArgs+=(--prefix PATH : ${lib.makeBinPath [ ffmpeg ]})
  '';

  meta = {
    description = "GTK4/libadwaita streaming frontend for mobile and desktop Linux";
    license = lib.licenses.gpl3Plus;
    mainProgram = "flux";
    platforms = lib.platforms.linux;
  };
}
