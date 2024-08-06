{
  description = "Anysnake2 generated flake";
  inputs = rec {

    nixpkgs = {
      url = "github:NixOS/nixpkgs?rev=ce6aa13369b667ac2542593170993504932eb836";

    };
    flake-utils = {
      url =
        "github:numtide/flake-utils?rev=7e5bf3925f6fbdfaf50a2a7ca0be2879c4261d19";

    };
    mach-nix = {
      url =
        "github:DavHau/mach-nix?rev=65266b5cc867fec2cb6a25409dd7cd12251f6107";
      inputs.pypi-deps-db.follows = "pypi-deps-db";

    };
    pypi-deps-db = {
      url =
        "github:DavHau/pypi-deps-db?rev=dd28bbd22df34cb9d00c8e48b21e74001c16b19d";
      inputs.mach-nix.follows = "mach-nix";

    };
  };

  outputs = { self, nixpkgs, flake-utils, mach-nix, pypi-deps-db }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        R_tracked = null;
        overlays = [];
        pkgs = import nixpkgs {
          inherit system overlays;
          config = { allowUnfree = false; };
        };
        mach-nix_ = (import mach-nix) {
          inherit pkgs;
          pypiDataRev = pypi-deps-db.rev;
          pypiDataSha256 = pypi-deps-db.narHash;
          python = "python310";
        };
        my_rust = "";

        _buildSymlinkImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { }, }:
          let
            mypy = if mach-nix_ != null then
              (mach-nix_.mkPython ({
                requirements = python_requirements;
                # no r packages here - we fix the rpy2 path below.
              } // additional_mkPythonArgs))
            else
              null;
            mypy2 = if mach-nix_ != null then (mypy.outPath) else "";
            script_file = pkgs.writeScript "reqs.sh" (mypy2 + "\n" + script);
          in {
            script_file = script_file;

            derivation = pkgs.runCommand name { } (''
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
            '' + (if R_tracked != null then ''
              # # is it smart to symlink the dependencies as well?
              for path in $(cat ${pkgs.writeReferencesToFile [ R_tracked ]});
                  do
              #    ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/bin $out/rootfs/bin/ || true
              #    ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/etc $out/rootfs/etc || true
              #    ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/lib $out/rootfs/usr/lib/ || true
              #    ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/share $out/rootfs/usr/share/ || true
                  ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/library $out/rootfs/R_libs/ || true
              done
            '' else
              "") + ''

                ln -s $out/rootfs/bin $out/rootfs/usr/bin
                #mkdir $out/python_env
                #ln -s $mypy2/* $out/python_env

                mkdir -p $out/rootfs/etc/profile.d
                echo "export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt" >>$out/rootfs/etc/bashrc # singularity pulls that from the env otherwise apperantly
                echo "export SSL_CERT_DIR=/etc/ssl/certs" >>$out/rootfs/etc/bashrc # singularity pulls that from the env otherwise apperantly
                #echo "export PATH=/python_env/bin:/bin:/usr/bin/" >>$out/rootfs/etc/bashrc
                #echo "export PYTHONPATH=$PYTHONPATH:/python_env/lib/python3.10/site-packages" >>$out/rootfs/etc/bashrc



              '');
          };

        buildSymlinkImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { }, }:
          (_buildSymlinkImage {
            inherit name script python_requirements additional_mkPythonArgs;
          }).derivation;

        buildSingularityImage =
          { name, script, python_requirements, additional_mkPythonArgs ? { }, }:
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
        python_requirements = ''
                  testrepo2==999+a42420f8ba0a6bc9bda0425cd665515fb92dc2b4
          testrepo==999+97d57e17c1bd4a5f547fa1c1be57c2f0a1d2ec6f
        '';

        _args = with pkgs; {
          #name = "first_container" + builtins.trace (pypi-deps-db.narHash) "";
          name = "anysnake2_container";
          inherit python_requirements;
          #later entries beat earlier entries in terms of /bin symlinks
          script = ''
                      ${coreutils}
                      ${bashInteractive_5}
                      ${my_rust}
                      ${which}
            ${cacert}

                      
          '';
          additional_mkPythonArgs = let
            input = {
              _."jupyter-core".postInstall = ''
                rm $out/lib/python*/site-packages/jupyter.py
                rm $out/lib/python*/site-packages/__pycache__/jupyter.cpython*.pyc
              '';
              # which is only going to work inside our container
              _."rpy2" = {
                postPatch = ''
                                #substituteInPlace 'rpy2/rinterface_lib/embedded.py' --replace '@NIX_R_LIBS_SITE@' "/R_libs"
                                substituteInPlace 'rpy2/rinterface_lib/embedded.py' --replace "os.environ['R_HOME'] = openrlib.R_HOME" \
                                "os.environ['R_HOME'] = openrlib.R_HOME
                          # path to libraries
                          existing = os.environ.get('R_LIBS_SITE')
                          if existing is not None:
                              prefix = existing + ':'
                          else:
                              prefix = '''
                          additional = '/R_libs'
                          os.environ['R_LIBS_SITE'] = prefix + additional
                  "
                                substituteInPlace 'requirements.txt' --replace 'pytest' ""
                '';
                patches = [ ];
              };
            } // (let
              testrepo_pkg = (mach-nix_.buildPythonPackage rec {
                pname = "testrepo";
                version = "999+97d57e17c1bd4a5f547fa1c1be57c2f0a1d2ec6f";
                src = pkgs.fetchFromGitHub { # testrepo
                  "owner" = "TyberiusPrime";
                  "repo" = "_anysnake2_test_repo";
                  "rev" = "97d57e17c1bd4a5f547fa1c1be57c2f0a1d2ec6f";
                  "sha256" =
                    "sha256-mZw37fLouWrA2L+49UOfUsF1MDy/q5pJImw+zczE4wU=";
                };

                overridesPre =
                  [ (self: super: { testrepo2 = testrepo2_pkg; }) ];
              });
              testrepo2_pkg = (mach-nix_.buildPythonPackage rec {
                pname = "testrepo2";
                version = "999+a42420f8ba0a6bc9bda0425cd665515fb92dc2b4";
                src = pkgs.fetchFromGitHub { # testrepo2
                  "owner" = "TyberiusPrime";
                  "repo" = "_anysnake2_test_repo2";
                  "rev" = "a42420f8ba0a6bc9bda0425cd665515fb92dc2b4";
                  "sha256" =
                    "sha256-tLz9vDTxQqFZPKkkBOZmmNNEhtf6JK2nwWiBKNH6od8=";
                };

              });
              machnix_overrides = (self: super: {
                testrepo = testrepo_pkg;
                testrepo2 = testrepo2_pkg;
              });
            in {
              packagesExtra = [ testrepo_pkg testrepo2_pkg ];
              overridesPre = [ machnix_overrides ];

              providers.testrepo = "nixpkgs";
              providers.testrepo2 = "nixpkgs";
            });
            override_func = old: old;
          in input // (override_func input);
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
          '' + (if R_tracked != null then ''
            export R_LIBS_SITE=${R_tracked}/lib/R/library/
          '' else
            "");
          nativeBuildInputs = with pkgs;
            [

            ];
        };
      });
}
