{
  description = "Anysnake2 generated flake";
  inputs = rec {
    #%INPUT_DEFS%
  };

  outputs = {
    self,
    #%INPUTS%
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system overlays;
        config = {
          allowUnfree = "%ALLOW_UNFREE%";
        };
      };
      #%DEFINITIONS%#
      _args = with pkgs; {
        name = "anysnake2_container";
        #later entries beat earlier entries in terms of /bin symlinks
        script = ''
          ${coreutils}
          ${bashInteractive_5}
          #%NIXPKGS_PACKAGES%#
        '';
      };
      helpers = import ./functions.nix {inherit pkgs;};
    in rec {
      defaultPackage = (helpers.buildSymlinkImage _args).derivation;
      oci_image = helpers.buildOCIimage _args;
      devShell = pkgs.stdenv.mkDerivation {
        name = "anysnake2-devshell";
        shellHook =
          ''
            export PATH=${defaultPackage}/rootfs/bin:$PATH;
            if test -f "develop_python_path.bash"; then
              source "develop_python_path.bash"
            fi
          ''
          + (
            if R_tracked != null
            then ''
              export R_LIBS_SITE=${R_tracked}/lib/R/library/
            ''
            else ""
          );
        nativeBuildInputs = with pkgs; [
          #%DEVSHELL_INPUTS%
        ];
      };
    });
}
