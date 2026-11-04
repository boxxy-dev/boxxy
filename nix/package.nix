{
  lib,
  rustPlatform,
  pkg-config,
  glib,
  gtk4,
  libadwaita,
  gtksourceview5,
  gst_all_1,
  wrapGAppsHook4,
  gitHash ? "unknown",
}:

let
  metadata = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "boxxy-terminal";
  version = metadata.workspace.package.version;

  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
  ];

  buildInputs = [
    glib
    gtk4
    libadwaita
    gtksourceview5
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
  ];

  buildFeatures = [
    "disable-self-update"
  ];

  cargoBuildFlags = [
    "-p"
    "boxxy-app"
    "-p"
    "boxxy-agent"
  ];

  # Bake the git commit hash into the binary so Preferences -> About,
  # the daemon version check, and the "What's New" dialog work in Nix builds.
  env = {
    GIT_HASH = gitHash;
  };

  doCheck = false;

  postInstall = ''
    install -Dm644 flatpak/dev.boxxy.BoxxyTerminal.desktop \
      $out/share/applications/dev.boxxy.BoxxyTerminal.desktop
    install -Dm644 flatpak/dev.boxxy.BoxxyTerminal.metainfo.xml \
      $out/share/metainfo/dev.boxxy.BoxxyTerminal.metainfo.xml
    install -Dm644 resources/icons/dev.boxxy.BoxxyTerminal.png \
      $out/share/icons/hicolor/256x256/apps/dev.boxxy.BoxxyTerminal.png
  '';

  meta = {
    description = metadata.workspace.package.description;
    homepage = "https://boxxy.dev";
    changelog = "https://boxxy.dev/releases/";
    license = lib.licenses.mit;
    mainProgram = "boxxy-terminal";
    platforms = lib.platforms.linux;
    maintainers = with lib.maintainers; [ mrdev023 ];
  };
}
