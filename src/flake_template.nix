{
  description = "Anysnake2 generated flake";
  inputs = rec {
    nipkgs.url = "%NIXPKG_URL%/?rev=%NIXPKG_REV%";
    flake-utils.url =
      "github:numtide/flake-utils?rev=7e5bf3925f6fbdfaf50a2a7ca0be2879c4261d19";
    rust-overlay = {
      url =
        "github:oxalica/rust-overlay?rev=23cce8b8a5ba7b01f69e344ea6f5e380988955f3";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
    mach-nix = {
      url =
        "github:DavHau/mach-nix/?rev=d884751a1c5942529ec8daf9d11ec516f8397b86";
      inputs.flake-utils.follows = "flake-utils";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.pypi-deps-db.follows = "pypi-deps-db";
    };
    pypi-deps-db = {
      url =
        "github:DavHau/pypi-deps-db/?rev=2d5a4a5dcc231bf7ade2cb141a9872f9f38f6e79";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.mach-nix.follows = "mach-nix";
    };
  };

  outputs =
    { self, nixpkgs, flake-utils, mach-nix, pypi-deps-db, rust-overlay }:

    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        mach-nix_ = (import mach-nix) {
          inherit pkgs;
          pypiDataRev = pypi-deps-db.rev;
          pypiDataSha256 = pypi-deps-db.narHash;
          python = "%PYTHON_MAJOR_MINOR%";
        };

        #buildContainer = (pkgs.callPackage ./buildContainer.nix {
        #inherit system;
        #mach-nix = mach-nix_;
        #}).buildSymlinkImage;
        buildContainer = { pkgs, lib, writeText, runCommand
          , writeReferencesToFile, mach-nix, system }:

          rec {
            _buildSymlinkImage = { name, script, python_requirements
              , additional_mkPythonArgs ? { } }:
              let
                mypy = mach-nix.mkPython ({
                  requirements = python_requirements;
                } // additional_mkPythonArgs);
                mypy2 =
                  mypy.outPath; # lib.strings.concatMapStrings (x: "{" + x + "}\n") (lib.attrNames (mypy ));
                script_file =
                  pkgs.writeScript "reqs.sh" (mypy2 + "\n" + script);
              in {
                script_file = script_file;
                derivation = pkgs.runCommand name { } ''
                  set -o pipefail
                  shopt -s nullglob
                  mkdir -p $out/rootfs/usr/lib
                  mkdir -p $out/rootfs/usr/share
                  cp ${script_file} $out/reqs.sh

                  # so singularity fills in the outside users
                  mkdir -p $out/rootfs/etc
                  touch $out/rootfs/etc/passwd
                  touch $out/rootfs/etc/group

                  mkdir -p $out/rootfs/bin
                  for path in "${mypy2}/bin/"*;
                    do
                      ln -s $path $out/rootfs/bin/
                  done

                  # the later entries shadow the earlier ones. and the python environment beats everything else
                  for path in $(tac ${script_file});
                     do
                     ln -s $path/bin/* $out/rootfs/bin/ || true
                     ln -s $path/lib/* $out/rootfs/usr/lib/ || true
                     ln -s $path/share/* $out/rootfs/usr/share/ || true
                  done
                '';
              };

            buildSymlinkImage = { name, script, python_requirements
              , additional_mkPythonArgs ? { } }:
              (_buildSymlinkImage {
                inherit name script python_requirements additional_mkPythonArgs;
              }).derivation;

            buildSingularityImage = { name, script, python_requirements
              , additional_mkPythonArgs ? { } }:
              let
                symlink_image = _buildSymlinkImage {
                  inherit name script python_requirements
                    additional_mkPythonArgs;
                };
              in pkgs.runCommand name { } ''
                set -o pipefail
                shopt -s nullglob
                mkdir -p $out/rootfs/

                ${pkgs.rsync}/bin/rsync -arW ${symlink_image.derivation}/rootfs/ $out/rootfs/
                chmod +w $out/rootfs -R # because we don't have write on the directories
                ${pkgs.rsync}/bin/rsync -arW --exclude=* --files-from=${
                  writeReferencesToFile [ symlink_image.script_file ]
                } / $out/rootfs/ 

                rm $out/rootfs/${symlink_image.script_file}
                chmod 755 $out/rootfs

                # # singularity tries to read resolv.conf, hosts and user definitions
                # # when converting the container
                # # so let's fake them
                mkdir $out/etc
                mkdir $out/build
                touch $out/etc/resolv.conf
                touch $out/etc/hosts
                echo "nixbld:x:1000:2000:imtseq:/home/installer:/bin/bash\n" >$out/etc/passwd
                echo "xxx:x: 2000:\n" >$out/etc/group
                echo ${pkgs.singularity}/bin/singularity
                ${pkgs.coreutils}/bin/whoami

                # # also consider NIX_REDIRECT and libredirect for this
                # the bash binding is needed for singularity to find 'sh'
                ${pkgs.bubblewrap}/bin/bwrap \
                   --proc /proc \
                   --dev /dev \
                  --bind $out/ $out/ \
                  --bind $out/build /build \
                  --ro-bind $out/etc /etc \
                  --ro-bind /nix /nix \
                  --ro-bind "${pkgs.bash}/bin" /usr/bin \
                  ${pkgs.singularity}/bin/singularity build  /build/${name}.sif $out/rootfs
                mv $out/build/*.sif $out/
                rm -rf $out/build
                rm -rf $out/etc
                # chmod +w $out/rootfs -R # because we don't have write on the directories
                # rm -rf $out/rootfs
              '';
          };
      in with pkgs; {
        defaultPackage = buildContainer {
          #name = "first_container" + builtins.trace (pypi-deps-db.narHash) "";
          name = "anysnake2_container";
          #later entries beat earlier entries in terms of /bin symlinks
          script = ''
            ${bashInteractive_5}
            %NIXPKGSPKGS%
            %RUST%
          '';
          python_requirements = ''
            %PYTHON_PACKAGES%
          '';
          additional_mkPythonArgs = {
            _."jupyter-core".postInstall = ''
              rm $out/lib/python*/site-packages/jupyter.py
              rm $out/lib/python*/site-packages/__pycache__/jupyter.cpython*.pyc
            '';
          };
        };
      });

}
