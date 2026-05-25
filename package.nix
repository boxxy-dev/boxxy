{
  lib,
  rustPlatform,

  pkg-config,
  openssl,
  glib,
  gtk4,
  libadwaita,
  gtksourceview5,
  gst_all_1,
  wrapGAppsHook4,
}:

let
  metadata = with builtins; (fromTOML (readFile ./Cargo.toml));
in
rustPlatform.buildRustPackage {
  pname = "boxxy";
  version = metadata.workspace.package.version;

  cargoLock.lockFile = ./Cargo.lock;
  src = ./.;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
  ];

  buildInputs = [
    openssl
    glib
    gtk4
    libadwaita
    gtksourceview5
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
  ];

  buildFeatures = [
    "disable-self-update"
  ];

  cargoBuildFlags = [
    "-p"
    "boxxy-terminal"
    "-p"
    "boxxy-agent"
  ];

  meta = {
    description = "Stupid Linux Terminal";
    mainProgram = "boxxy";
    homepage = "https://github.com/boxxy-dev/boxxy";
    platforms = lib.platforms.linux;
    maintainers = with lib.maintainers; [ mrdev023 ];
  };
}
