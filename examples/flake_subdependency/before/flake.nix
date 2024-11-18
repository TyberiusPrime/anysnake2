{
  description = "Anysnake2 generated flake";
  inputs = rec {

    flake-utils = {
      url =
        "github:numtide/flake-utils/b1d9ab70662946ef0850d488da1c9019f3a9752a";

    };
    nixpkgs = {
      url = "github:NixOS/nixpkgs/24.05";

    };
    rust-overlay = {
      url =
        "github:oxalica/rust-overlay/0be641045af6d8666c11c2c40e45ffc9667839b5";
      inputs.nixpkgs.follows = "nixpkgs";

    };
    test = {
      url =
        "github:nix-community/poetry2nix/8810f7d31d4d8372f764d567ea140270745fe173";

    };
  };

  outputs = { self, flake-utils, nixpkgs, rust-overlay, test }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system overlays;
          config = { allowUnfree = false; };
        };
        R_tracked = null;
        overlays = [ (import rust-overlay) ];
        rust = pkgs.rust-bin.stable."1.55.0".minimal.override {
          extensions = [ "rustfmt" "clippy" ];
        };
        _args = with pkgs; {
          name = "anysnake2_container";
          #later entries beat earlier entries in terms of /bin symlinks
          script = ''
                      ${coreutils}
                      ${bashInteractive_5}
                      ${bash}
            ${cacert}
            ${fish}
            ${rust}
            ${stdenv.cc}
          '';
        };
        helpers = import ./functions.nix { inherit pkgs; };
      in rec {
        defaultPackage = (helpers.buildSymlinkImage _args).derivation;
        oci_image = helpers.buildOCIimage _args;
        devShell = pkgs.stdenv.mkDerivation {
          name = "anysnake2-devshell";
          shellHook = ''
            export PATH=${defaultPackage}/rootfs/bin:$PATH;
            if test -f "develop_python_path.bash"; then
              source "develop_python_path.bash"
            fi
          '' + (if R_tracked != null then ''
            export R_LIBS_SITE=${R_tracked}/lib/R/library/
          '' else
            "");
          nativeBuildInputs = with pkgs;
            [
              #%DEVSHELL_INPUTS%
            ];
        };
      });
}
