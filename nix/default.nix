{
  pkgs ? import <nixpkgs> { },
  gitHash ? "unknown",
}:

pkgs.callPackage ./package.nix { inherit gitHash; }
