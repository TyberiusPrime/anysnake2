{
  description = "Anysnake2 generated flake";
  inputs = rec {
    #%INPUT_DEFS%
  };

  outputs = { self,
    #%INPUTS%
    }:

    flake-utils.lib.eachDefaultSystem (system:
      let
        #%RPACKAGES%
        overlays = "%OVERLAY_AND_PACKAGES%";
        pkgs = import nixpkgs { inherit system overlays; };
        mach-nix_ = "%MACHNIX%";
        my_rust = "%RUST%";

        _buildSymlinkImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { } }:
          let
            mypy = if mach-nix_ != null then
              (mach-nix_.mkPython ({
                requirements = python_requirements;
                #%MACHNIX_PKG_EXTRAS%
              } // additional_mkPythonArgs))
            else
              null;
            mypy2 = if mach-nix_ != null then (mypy.outPath) else "";
            script_file = pkgs.writeScript "reqs.sh"
              (mypy2 + "\n" + script);
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

              mkdir -p $out/rootfs/{bin,etc,share}
              mkdir -p $out/rootfs/usr/{lib/share}
              mkdir -p $out/rootfs/R_libs

              # the later entries shadow the earlier ones. and the python environment beats everything else
              set -x
              # python packages beat the others# python packages beat the others..
              if [ -n "${mypy2}" ]; then
                ${pkgs.xorg.lndir}/bin/lndir -ignorelinks ${mypy2}/bin $out/rootfs/bin/ || true
              fi


              # symlink the direct dependencies first...
              for path in $(tac ${script_file});
                 do
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/bin $out/rootfs/bin/ || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/etc $out/rootfs/etc || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/lib $out/rootfs/usr/lib/ || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/share $out/rootfs/usr/share/ || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/library $out/rootfs/R_libs/ || true
              done
              # is it smart to symlink the dependencies as well?
              for path in $(cat ${pkgs.writeReferencesToFile [ script_file ]});
                 do
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/bin $out/rootfs/bin/ || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/etc $out/rootfs/etc || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/lib $out/rootfs/usr/lib/ || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/share $out/rootfs/usr/share/ || true
                 ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/library $out/rootfs/R_libs/ || true
              done

              ln -s $out/rootfs/bin $out/rootfs/usr/bin
              #mkdir $out/python_env
              #ln -s $mypy2/* $out/python_env

              mkdir -p $out/rootfs/etc/profile.d
              echo "export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt" >>$out/rootfs/etc/bashrc # singularity pulls that from the env otherwise apperantly
              echo "export SSL_CERT_DIR=/etc/ssl/certs" >>$out/rootfs/etc/bashrc # singularity pulls that from the env otherwise apperantly
              #echo "export PATH=/python_env/bin:/bin:/usr/bin/" >>$out/rootfs/etc/bashrc 
              #echo "export PYTHONPATH=$PYTHONPATH:/python_env/lib/python%PYTHON_MAJOR_DOT_MINOR%/site-packages" >>$out/rootfs/etc/bashrc 

              #%INSTALL_JUPYTER_KERNELS%

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
            #make sure we got everything from the nix store, right?
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
            %NIXPKGS_PACKAGES%
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
#            _."rpy2".RPY2_CFFI_MODE = "API";
          };
        };
      in rec {
        defaultPackage = buildSymlinkImage _args;
        sif_image = buildSingularityImage _args;
        devShell = pkgs.stdenv.mkDerivation {
          name = "anysnake2-devshell";
          shellHook = ''
            export PATH=${defaultPackage}/rootfs/bin:$PATH;
            if test -f "develop_python_path.bash"; then
              source "develop_python_path.bash"
            fi 
          '';
          nativeBuildInputs = with pkgs;
            [
              #%DEVSHELL_INPUTS%
            ];
        };
      });

}
