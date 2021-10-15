{
  description = "Anysnake2 generated flake";
  inputs = rec {
    nixpkgs.url = "%NIXPKG_URL%/?rev=%NIXPKG_REV%";
    flake-utils.url = "github:numtide/flake-utils?rev=%FLAKE_UTIL_REV%";
    rust-overlay = {
      url = "%RUST_OVERLAY_URL%?rev=%RUST_OVERLAY_REV%";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
    mach-nix = {
      url = "%MACH_NIX_URL%/?rev=%MACH_NIX_REV%";
      inputs.flake-utils.follows = "flake-utils";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.pypi-deps-db.follows = "pypi-deps-db";
    };
    pypi-deps-db = {
      url = "github:DavHau/pypi-deps-db/?rev=%PYPI_DEPS_DB_REV%";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.mach-nix.follows = "mach-nix";
    };
    #%FURTHER_FLAKES%
  };

  outputs = { self, nixpkgs, flake-utils, mach-nix, pypi-deps-db, rust-overlay,
    #%FURTHER_FLAKE_PARAMS% 
    }:

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

        my_rust = "%RUST%";

        _buildSymlinkImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { } }:
          let
            mypy = mach-nix_.mkPython ({
              requirements = python_requirements;
            } // additional_mkPythonArgs);
            mypy2 =
              mypy.outPath; # lib.strings.concatMapStrings (x: "{" + x + "}\n") (lib.attrNames (mypy ));
            script_file = pkgs.writeScript "reqs.sh" (mypy2 + "\n" + script);
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

        buildSymlinkImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { } }:
          (_buildSymlinkImage {
            inherit name script python_requirements additional_mkPythonArgs;
          }).derivation;

        buildSingularityImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { } }:
          let
            symlink_image = _buildSymlinkImage {
              inherit name script python_requirements additional_mkPythonArgs;
            };
          in pkgs.runCommand name { } ''
            set -o pipefail
            shopt -s nullglob
            mkdir -p $out/rootfs/

            ${pkgs.rsync}/bin/rsync -arW ${symlink_image.derivation}/rootfs/ $out/rootfs/
            chmod +w $out/rootfs -R # because we don't have write on the directories
            ${pkgs.rsync}/bin/rsync -arW --exclude=* --files-from=${
              pkgs.writeReferencesToFile [ symlink_image.script_file ]
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
        _args = with pkgs; {
          #name = "first_container" + builtins.trace (pypi-deps-db.narHash) "";
          name = "anysnake2_container";
          #later entries beat earlier entries in terms of /bin symlinks
          script = ''
            ${coreutils}
            ${bashInteractive_5}
            ${my_rust}
            %NIXPKGSPKGS%
            %FURTHER_FLAKE_PACKAGES%
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
      in { 
        defaultPackage = buildSymlinkImage _args; 
        image = buildSingularityImage _args; 
      });

}
