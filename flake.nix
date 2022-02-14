{
  inputs = {
    nixpkgs.url =
      "github:NixOS/nixpkgs?rev=7e9b0dff974c89e070da1ad85713ff3c20b0ca97"; # that's 21.05
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

  outputs = { self, nixpkgs, utils, naersk, rust-overlay }:
    utils.lib.eachDefaultSystem (system:
      let
        #pkgs = nixpkgs.legacyPackages."${system}";

        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rust = pkgs.rust-bin.stable."1.58.1".default.override {
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        # Override the version used in naersk
        naersk-lib = naersk.lib."${system}".override {
          cargo = rust;
          rustc = rust;
        };

        bacon = naersk-lib.buildPackage { # could also pull a slightly older one from nixpkgs
          pname = "bacon";
          version = "1.2.5";
          src = pkgs.fetchFromGitHub {
            owner = "Canop";
            repo = "bacon";
            rev = "0077701f2923a43d7c37f9e532163bfa01af6b1c";
            sha256 = "sha256-dpdQ1qBfLU6whkqVHQ/zQxqs/y+nmdvxHanaNw66QxA=";
          };
        };

      in rec {
        # `nix build`
        packages.my-project = naersk-lib.buildPackage {
          pname = "anysnake2";
          root = ./.;
        };
        defaultPackage = packages.my-project;

        # `nix run`
        apps.my-project = utils.lib.mkApp { drv = packages.my-project; };
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
