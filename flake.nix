{
  description = "Boxxy Terminal - A modern Linux terminal emulator with AI assistant";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      gitHash = self.shortRev or self.dirtyShortRev or "unknown";
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          boxxy = pkgs.callPackage ./nix/package.nix { inherit gitHash; };
        in
        {
          default = boxxy;
          boxxy = boxxy;
          boxxy-terminal = boxxy;
        }
      );

      overlays.default = final: prev: {
        boxxy-terminal = final.callPackage ./nix/package.nix { inherit gitHash; };
      };
    };
}
