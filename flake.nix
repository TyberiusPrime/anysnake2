{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs?ref=24.05"; # that's 23.05
    utils.url = "github:numtide/flake-utils";
    utils.inputs.nixpkgs.follows = "nixpkgs";
    naersk.url = "github:nmattia/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

    ancient-poetry.url = "git+https://codeberg.org/TyberiusPrime/ancient-poetry.git";
    ancient-poetry.inputs.nixpkgs.follows = "nixpkgs";
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
    ancient-poetry,
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
          pkgs.cargo-crev
          pkgs.cargo-vet
          pkgs.cargo-outdated
          pkgs.cargo-audit
          ancient-poetry.defaultPackage.x86_64-linux
          bacon
        ];
      };
    });
}
# {

