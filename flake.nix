{
  description = "Unofficial local-first Garmin toolkit";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane = {
      url = "github:ipetkov/crane";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    desktop = {
      url = "path:./apps/garmin-desktop";
      inputs = {
        crane.follows = "crane";
        nixpkgs.follows = "nixpkgs";
        rust-overlay.follows = "rust-overlay";
      };
    };

    gallery = {
      url = "path:./infra/gallery";
      inputs = {
        crane.follows = "crane";
        nixpkgs.follows = "nixpkgs";
        rust-overlay.follows = "rust-overlay";
      };
    };

    hass = {
      url = "path:./apps/garmin-hass";
      inputs = {
        crane.follows = "crane";
        nixpkgs.follows = "nixpkgs";
        rust-overlay.follows = "rust-overlay";
      };
    };

    # Keep nix-tools' pinned nixpkgs: its tools and configs are a matched set.
    nix-tools.url = "github:kubijo/nix-tools";
  };

  outputs = inputs: import ./infra/nix/flake.nix inputs;
}
