nix shell \
    github:NixOS/nixpkgs/24.05#dtach \
    -c \
    dtach \
    -c \
    .anysnake2_flake/dtach/run_2024-11-18_15:03:22 \
    nix \
    shell \
    github:NixOS/nixpkgs/24.05#singularity \
    -c \
    singularity exec \
    --userns \
    --cleanenv \
    --home /home/finkernagel \
    --bind /nix/store:/nix/store:ro \
    --bind .anysnake2_flake/run_scripts/run/run.sh:/anysnake2/run.sh:ro \
    --bind .anysnake2_flake/run_scripts/run/post_run.sh:/anysnake2/post_run.sh:ro \
    --bind .anysnake2_flake/run_scripts/run/outer_run.sh:/anysnake2/outer_run.sh:ro \
    --bind /home/finkernagel/upstream/anysnake2/examples/flake_subdependency:/project:rw \
    --env ANYSNAKE2=1 \
    --env PATH=/bin \
    .anysnake2_flake/result/rootfs \
    /bin/bash \
    /anysnake2/outer_run.sh \
