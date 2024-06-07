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
    in {
      defaultPackage = (helpers.buildSymlinkImage _args).derivation;
      sif_image = helpers.buildSingularityImage _args;
    });
}
