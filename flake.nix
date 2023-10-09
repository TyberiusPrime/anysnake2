{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs?rev=235aaea29ad2d58623679fd1409bcf01c19502be"; # that's 23.05
    utils.url = "github:numtide/flake-utils";
    utils.inputs.nixpkgs.follows = "nixpkgs";
    naersk.url = "github:nmattia/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    #mozillapkgs = {
    #url = "github:mozilla/nixpkgs-mozilla";
    #flake = false;
    #};
  };

  outputs = {
    self,
    nixpkgs,
    utils,
    naersk,
    rust-overlay,
  }:
    utils.lib.eachDefaultSystem (system: let
      #pkgs = nixpkgs.legacyPackages."${system}";
      overlays = [(import rust-overlay)];
      pkgs = import nixpkgs {inherit system overlays;};
      rust = pkgs.rust-bin.stable."1.72.0".default.override {
        targets = ["x86_64-unknown-linux-musl"];
      };

      # Override the version used in naersk
      naersk-lib = naersk.lib."${system}".override {
        cargo = rust;
        rustc = rust;
      };

      bacon = pkgs.bacon;
    in rec {
      # `nix build`
      packages.my-project = naersk-lib.buildPackage {
        pname = "anysnake2";
        root = ./.;
      };
      defaultPackage = packages.my-project;

      # `nix run`
      apps.my-project = utils.lib.mkApp {drv = packages.my-project;};
      defaultApp = apps.my-project;

      # `nix develop`
      devShell = pkgs.mkShell {
        # supply the specific rust version
        nativeBuildInputs = [
          rust
          pkgs.rust-analyzer
          pkgs.git
          pkgs.cargo-udeps
          pkgs.cargo-audit
          bacon
        ];
      };
    });
}
# {

