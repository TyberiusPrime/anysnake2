## Anysnake2

Fully reproducible C/Python/R/Rust environments for research.

Anysnake2 levers [Nix](https://nixos.org/), [mach-nix](https://github.com/DavHau/mach-nix), 
[r_ecosystem_track](https://github.com/TyberiusPrime/r_ecosystem_track) and [rust-overlay](https://github.com/oxalica/rust-overlay)
to give you 'virtual environments' that are fully define define in an easy to use [toml](https://github.com/toml-lang/toml) file.

# How it works

The first thing the anysnake2 does is read the anysnake2 version from your config file (./anysnake2.toml by default).
It then restarts itself with that exact anysnake2 version using Nix.

Next it writes a [Nix flake](https://nixos.wiki/wiki/Flakes), and turns it into either a symlink forest that works
as a rootless singularity container, or optionally a container image in SIF (singularity) format.

Last it runs a bash script inside that container for you. This can be an analysis script, a shell, jupyter, whatever you want.

The advantage here is that the process is deterministic - do it again on another machine and you will get the exact same
container (unlike e.g. Dockerfiles). It's also incremental, with very efficient caching thanks to nix, so a new project
with slight tweaks will not take an hour to build. And unlike Conda you're not restricted to R & Python, while at the
same time insulating you from the underlying c ecosystem (=linux distribution).

# Background

Nix is a package manager and language to describe fully reproducible builds using 'build recipes'.

Nix flakes on top make the recipes themselves fully reproducible, by 'locking' hashes and restricting
the functionality of Nix lang to be 'hermetic', ie. self contained. Every downloaded file and the recepies are verified with hashes.
(A side effect here is of course that if the upstream file is replaced, your installation will fail loudly - and that's 
the only way to loose the reproducibility).

Using just a fixed version of nixpkgs, the community recipe collection in a flake, you already have a reproducible environment.
But it is limited to the versions of everything present in that nixpkgs revision. Anysnake2 integrates other projects
to introduce flexibility.

For example, [mach-nix](https://github.com/DavHau/mach-nix) offers you reproducible python package dependency resolution, by
a simple trick: You need to specify the date (=[pypi-deps-db](https://github.com/DavHau/pypi-deps-db) date) of the python ecosystem to use. Once that's defined, the dependency
resolution and which exact packages to install is fixed.  Anysnake2 improves on this be letting you specify a date

R_ecosystem_track offers something similar for R.
Rust-overlay let's you include arbitrary rust versions. And you can extend the flake with any other flake you like.

# Prerequisites

You need a working nix installation with flakes.

If you're using NixOS, referer to the [nix wiki](https://nixos.wiki/wiki/Flakes#NixOS)
Otherwise, you could use the  [nix-unstable installer](https://github.com/numtide/nix-unstable-installer).

The following examples use nix to temporarily install anysnake2 (until your next nix-collect-garbage),
see the [installation section](# Installation)

# Getting started.

Run `nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 config basic >anysnake2.toml` and
have a look at the resulting toml file, which contains all the basic configuration (use `config full` for an example
containing every option).

You should find something close to this.

```toml
[anysnake2]
rev = "1.0"

[outside_nixpkgs]
# the nixpkgs used to run singularity and nixfmt
rev = "21.05"

[nixpkgs]
# the nixpkgs used inside the container
rev = "21.05" # the nixpgks version or github hash
packages = [ # use https://search.nixos.org/packages to search
	"fish",
]

[python] # python section is optional
version="3.8" # does not go down to 3.8.x. Thats implicit in the nixpkgs (for now)
ecosystem_date="2021-04-12" # you get whatever packages the solver would have produced on that day

[python.packages]
jupyter=""


[container.volumes_rw]
"." = "/project" # map the current folder to /project

[env]
ANYSNAKE2=1

[cmd.default]
run = """
cd /project
jupyter notebook
"""

[cmd.shell]
run = """fish
"""

```

This defines an anysnake2 project that uses nixpkgs 21.05, both inside the container, and outside 
(which defines the singularity version and auxiliaries it uses), python from 2021-04-12, and neither R nor Rust.

If you now run `nix shell "github:TyberiusPrime/anysnake2" -c anysnake2`,
you'll get a jupyter notebook running after some downloading. But the next time
will be very quick. (See the "singularity won't run" section below if
singularity complains about `failed to resolve session directory
/var/singularity/mnt/session`).

Note that by default, singularity maps your home into the container

For example, try adding 'pandas>=1.2' to the list of python packages above, and restart the nix shell command. 
You'll find that pandas 1.2.4 has been installed. That's not the most recent release - but it was at the python
ecosystem date we've been using. Change that to just a day later, and rerun `nix shell ...`,  and you'll get 
pandas 1.3.0 instead.

Instead of the default command (which is defined by cmd.default) you can also run a custom command with an arbitrary
name (minus some build-in-exclusions), like the `shell` command above. 
`nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 shell`, will execute a fish shell inside your container.


# Using R

(Note: r_ecosystem_track is not ready yet, and not integrated into anysnake2 as of 2021-19-10,
but this is how it's going to work).

Including R and R packages using r_ecosystem_track is even simpler than python packages,
since you will get whatever R, bioconductor and package version was current at a particular
ecosystem date.

Just include this:

```toml
[R]
ecosystem_date="2021-10-11" # you get whatever packages were current that day.
packages = [
	"ggplot2",
]
```


# Using rust
```toml
[rust]
version = "1.55.0" # =stable. 
# to use nightly, add for examle this nixpkgs.packages 'rust-bin.nightly."2020-01-01".default'
```

# Other flakes
Include other flakes like this.
Note that we do not rely on flake.lock - I found it to have a tendency to update dependencies when 
you were not expecting it to.

```toml
[flakes.hello]
	url = "github:/TyberiusPrime/hello_flake" #https://nixos.wiki/wiki/Flakes#Input_schema - relative paths are tricky
	rev = "f32e7e451e9463667f6a1ddb7a662ec70d35144b" # flakes.lock tends to update unexpectedly, so we tie it down here
	follows = ["nixpkgs"] # so we overwrite the flakes dependencies
	packages = ["defaultPackage.x86_64-linux"]
```


# Clones and editable python installs

Anysnake2 can be used to clone repositories you want to work on into this project folder,
and, optionally, include them in your python search path inside the container.


For example to clone into the 'code' directory, use this

```toml
[clones.code] # target directory
# seperate from python packages so you can clone other stuff as well
dppd="git+https://github.com/TyberiusPrime/dppd
```

You can use `[clone_regexps]` to save on typing here - see [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml). The cloning happens only if the target folder does not exist yet (no automatic pull).

You can then include this package in your python packages list like this
`dppd=editable/code`. Anysnake2 will then (once) run a container that 
runs `pip install -e .` on that (possibly cloned) folder, and include it
in the containers python path. It will also, on every run, parse the 
requirements.txt/setup.cfg of the package and add it's requirements to
the python packages to resolve & install.


# Container runtime

Anysnake2 uses [singularity](https://singularity.hpcng.org/) as a container runtime,
since it offers rootless containers that can run from locally
unpacked images (running from an image file unfortunately requires root and 
the +s binary is not available using nix).

The actual run command is printed out on every run, and also stored in 'flake/run_scripts/<cmd>/singularity.bash'.

You can influence the mounted volumes using `[container.volumes_ro]` for read only and `[container.volumes_rw`] for read/write
volumes. Environment variables can be set using the `[container.env]` section

The actual commands can be surrounded by pre/post run commands outside/inside the container - see the 
[full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml). 

Build in commands (which you can not replace by config) are 

 * `build rootfs` - just build the (unpacked) container as a symlink tree
 * `build sif` - build the container image in flake/result/anysnake2_container.sif
 * `config` - list the available example configs (and config <name> to print one)
 * `help` - help
 * `version` - output anysnake2 version
 * `run --` - run arbitrary commands (without pre/post wrappers). Everything after -- is passed on to the container


# Singularity won't run

Singularity needs some folders that are apparently not wrapped by nixpkgs.
You'll need to create them: `sudo mkdir -p /var/singularity/mnt/{container,final,overlay,session}`.


# Version policy

Anysnake2 will follow semver once 1.0 is reached.
But with the auto-use-the-specified-version-mechanism, it's a bit of a moot point.


# Installation

Either add this repository as a flake to your nix configuration,
or download one of the prebuild binaries (which are statically linked against musl).
