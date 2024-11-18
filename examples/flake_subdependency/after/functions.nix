{pkgs}: rec {
  buildSymlinkImage = {
    name,
    script,
  }: let
  in rec {
    script_file = pkgs.writeScript "reqs.sh" script;
    derivation = pkgs.runCommand "${name}-2" {} ''
      set -o pipefail
      shopt -s nullglob
      mkdir -p $out/rootfs/usr/lib
      mkdir -p $out/rootfs/usr/share
      cp ${script_file} $out/reqs.sh

      # so singularity fills in the outside users
      mkdir -p $out/rootfs/etc
      touch $out/rootfs/etc/passwd
      touch $out/rootfs/etc/group

      mkdir -p $out/rootfs/{bin,etc,share,tmp,var/tmp}
      mkdir -p $out/rootfs/usr/lib
      mkdir -p $out/rootfs/usr/lib/share
      mkdir -p $out/rootfs/R_libs

      set -x
      # the later entries shadow the earlier ones.
      # symlink the direct dependencies ...
      for path in $(tac ${script_file});
         do
         ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/bin $out/rootfs/bin/ || true
         ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/etc $out/rootfs/etc || true
         ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/lib $out/rootfs/usr/lib/ || true
         ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/share $out/rootfs/usr/share/ || true
         if [ -d $path/lib/R/library/ ]; then
           ${pkgs.xorg.lndir}/bin/lndir -ignorelinks $path/lib/R/library $out/rootfs/R_libs/ || true
         fi
      done

      ln -s $out/rootfs/bin $out/rootfs/usr/bin

      mkdir -p $out/rootfs/etc/profile.d
      echo "export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt" >>$out/rootfs/etc/bashrc # singularity pulls that from the env otherwise apperantly
      echo "export SSL_CERT_DIR=/etc/ssl/certs" >>$out/rootfs/etc/bashrc # singularity pulls that from the env otherwise apperantly
      #echo "export PATH=/python_env/bin:/bin:/usr/bin/" >>$out/rootfs/etc/bashrc
      #echo "export PYTHONPATH=$PYTHONPATH:/python_env/lib/python%PYTHON_MAJOR_DOT_MINOR%/site-packages" >>$out/rootfs/etc/bashrc


    '';
  };

  # can't use pkgs.ociTools.buildContainerImage
  # because it a) does not work from a rootfs
  # and b) doesn't actually build an image, just a runtime bundle
  buildOCIimage = {
    name,
    script,
  }: let
    symlink_image = buildSymlinkImage {
      inherit name script;
    };
    umoci = pkgs.umoci;
  in
    pkgs.runCommand "${name}.oci" {} ''
    set -o pipefail
    shopt -s nullglob
    ${umoci}/bin/umoci init --layout "${name}"
    ${umoci}/bin/umoci new --image "${name}:latest"
    mkdir tmp-oci-unpack

    mkdir fakeroot/etc -p
    touch fakeroot/etc/resolv.conf

    # umoci tries to read /etc/resolv.conf, so let's give it one..
    ${pkgs.bubblewrap}/bin/bwrap \
           --proc /proc \
            --dev /dev \
           --bind /build /build \
           --ro-bind /build/fakeroot/etc /etc \
           --ro-bind /nix /nix \
      ${umoci}/bin/umoci unpack --image "${name}:latest" tmp-oci-unpack --rootless

    echo "rsyncing symlink forest"
    ${pkgs.rsync}/bin/rsync -arW ${symlink_image.derivation}/rootfs/ tmp-oci-unpack/rootfs/
    chmod +w tmp-oci-unpack/rootfs -R # because we don't have write on the directories

    echo "rsyncing necessary nix paths"
    ${pkgs.rsync}/bin/rsync -arW --exclude=* --files-from=${
        pkgs.writeClosure [symlink_image.script_file]
      } / tmp-oci-unpack/rootfs/


    # check that we have a rootfs/bin/sh
    if [ ! -e tmp-oci-unpack/rootfs/bin/sh ]; then
      echo "No rootfs/bin/sh found"
      exit 1
    fi
    chmod +w tmp-oci-unpack/rootfs -R # because we don't have write on the directories

    ${umoci}/bin/umoci repack --image "${name}:latest" tmp-oci-unpack
    ${umoci}/bin/umoci config --image "${name}:latest"
    # strip the first component
    cd "${name}" && tar cf $out .


    '';

  # this never quite worked and since like 22.05, singularity image building on nixos seems broken
  #   buildSingularityImage = {
  #     name,
  #     script,
  #   }: let
  #     symlink_image = buildSymlinkImage {
  #       inherit name script;
  #     };
  #   in
  #     pkgs.runCommand name {} ''
  #       set -o pipefail
  #       shopt -s nullglob
  #       mkdir -p $out/rootfs/

  #       ${pkgs.rsync}/bin/rsync -arW ${symlink_image.derivation}/rootfs/ $out/rootfs/
  #       chmod +w $out/rootfs -R # because we don't have write on the directories
  #       #make sure we got everything from the nix store, right?
  #       ${pkgs.rsync}/bin/rsync -arW --exclude=* --files-from=${
  #         pkgs.writeClosure [symlink_image.script_file]
  #       } / $out/rootfs/

  #       rm $out/rootfs/${symlink_image.script_file}
  #       chmod 755 $out/rootfs

  #       # # singularity tries to read resolv.conf, hosts and user definitions
  #       # # when converting the container
  #       # # so let's fake them
  #       mkdir $out/etc
  #       mkdir $out/build
  #       touch $out/etc/resolv.conf
  #       touch $out/etc/hosts
  #       echo "nixbld:x:1000:2000:imtseq:/home/installer:/bin/bash\n" >$out/etc/passwd
  #       echo "xxx:x: 2000:\n" >$out/etc/group
  #       echo ${pkgs.singularity}/bin/singularity
  #       ${pkgs.coreutils}/bin/whoami

  #       # # also consider NIX_REDIRECT and libredirect for this
  #       # the bash binding is needed for singularity to find 'sh'
  #       ${pkgs.bubblewrap}/bin/bwrap \
  #          --proc /proc \
  #          --dev /dev \
  #         --bind $out/ $out/ \
  #         --bind $out/build /build \
  #         --ro-bind $out/etc /etc \
  #         --ro-bind /nix /nix \
  #         --ro-bind "${pkgs.bash}/bin" /usr/bin \
  #         ${pkgs.singularity}/bin/singularity build  /build/${name}.sif $out/rootfs -v
  #       mv $out/build/*.sif $out/
  #       rm -rf $out/build
  #       rm -rf $out/etc
  #       # chmod +w $out/rootfs -R # because we don't have write on the directories
  #       # rm -rf $out/rootfs
  #     '';
}
