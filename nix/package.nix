{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  gtk4,
  libadwaita,
  glib-networking,
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
      ../data
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
    # TLS for GStreamer's HTTP source (libsoup).
    glib-networking
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

  postInstall = ''
    install -Dm644 data/io.github.patwid.Flux.desktop -t $out/share/applications
    install -Dm644 data/io.github.patwid.Flux.metainfo.xml -t $out/share/metainfo
    install -Dm644 data/icons/io.github.patwid.Flux.svg -t $out/share/icons/hicolor/scalable/apps
  '';

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
