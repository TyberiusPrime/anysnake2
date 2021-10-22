## Anysnake2

Fully reproducible C/Python/R/Rust environments for research.

Anysnake2 levers [Nix](https://nixos.org/), [mach-nix](https://github.com/DavHau/mach-nix), 
[r_ecosystem_track](https://github.com/TyberiusPrime/r_ecosystem_track) and [rust-overlay](https://github.com/oxalica/rust-overlay)
to give you 'virtual environments' that are fully defined in an easy to use [toml](https://github.com/toml-lang/toml) file.

# How it works

The first thing the anysnake2 does is read the anysnake2 version from your project config file.
It then restarts itself with that exact anysnake2 version using Nix.

Next it writes a [Nix flake](https://nixos.wiki/wiki/Flakes), and turns it into either a symlink forest that works
as a rootless singularity container.

Last it extracts container settings from the config file and runs a bash script inside the container for you. 
This can be an analysis script, a shell, jupyter, whatever you want.

The advantage here is that the process is deterministic - do it again on another machine and you will get the exact same
container (unlike e.g. Dockerfiles). It's also incremental, with very efficient caching thanks to Nix, so a new project
with slight tweaks will not take an hour to build. And unlike Conda you're not restricted to R & Python, while at the
same time insulating you from the underlying c ecosystem (=linux distribution).

# Background

Nix is a package manager and language to describe fully reproducible builds using 'build recipes'.

Nix flakes on top make the recipes themselves fully reproducible, by 'locking' hashes and restricting
the functionality of Nix lang to be 'hermetic', ie. self contained. Every downloaded file and the recipes are verified with hashes.
(A side effect here is of course that if the upstream file is changed, your installation will fail loudly - and that's 
about the only way to loose the reproducibility).

Using just a fixed version of nixpkgs (the community recipe collection) in a flake, you already have a reproducible environment.

But it is limited to the versions of everything present in that nixpkgs
revision. Anysnake2 integrates other projects to introduce flexibility.

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
containing every option. anysnake2.toml is the default config filename).

You should find something close to this.

```toml
# package settings
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
version="3.8" # does not go down to 3.8.x. That's implicit in the nixpkgs (for now)
ecosystem_date="2021-04-12" # you get whatever packages the solver would have produced on that day

[python.packages]
# you can use version specifiers from https://www.python.org/dev/peps/pep-0440/#id53
jupyter="" 


# container settings
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
(which defines the singularity version and auxiliaries used), python from 2021-04-12, and neither R nor Rust.

If you now run `nix shell "github:TyberiusPrime/anysnake2" -c anysnake2`,
you'll get a jupyter notebook running after some downloading. But the next time
will be very quick. (See the "singularity won't run" section below if
singularity complains about `failed to resolve session directory
/var/singularity/mnt/session`).

Note that by default, singularity maps your home into the container. You can adjust that with the container.home setting
(see [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml)).

For example, try adding 'pandas>=1.2' to the list of python packages above, and restart the nix shell command. 
You'll find that pandas 1.2.4 has been installed. That's not the most recent release - but it was at the python
ecosystem date we've been using. Change that to just a day later, and rerun `nix shell ...`,  and you'll get 
pandas 1.3.0 instead.

(A quick way to check the pandas version is
nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 run -- python -c "'import pandas; print(pandas.__version__)'".
Yes the escaping between nix shell, anysnake and python in series is a bit of a mess.)


Instead of the default command (which is defined by cmd.default in the config toml) you can also run a custom command with an arbitrary
name (minus some build-in-exclusions), like the `shell` command defined above.
`nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 shell`, will execute a fish shell inside your container.


# Installation

...


# Using R

(Note: r_ecosystem_track is not ready yet, and not integrated into anysnake2 as of 2021-19-10,
but this is how it's going to work).

Including R and R packages using
[r_ecosystem_track](https://github.com/TyberiusPrime/r_ecosystem_track) is even
simpler than python packages, since you will get whatever R, bioconductor and
package version was current at a particular ecosystem date.

Just include this:

```toml
[R]
# you get whatever packages were current that day.
r_ecosystem_track_rev="2021-10-11_v1"  # a tag or revision from the r_ecosystem_track repo
packages = [
	"ggplot2",
]
```

(as of 2021-10-18, r_ecosystem_track is not ready yet, and there is no R integration.)


# Using rust
```toml
[rust]
version = "1.55.0" # =stable.
# to use nightly, add for example this:
# [nixpkgs]
# packages = ['rust-bin.nightly."2020-01-01".default']
```

# Other flakes
Include other flakes like this.
Note that we do not rely on flake.lock, so you have to define a revision/tag. 
I've found nix flakes to have a tendency to update locked dependencies when
you were not expecting it to do so.

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

You can use `[clone_regexps]` to save on typing here - see [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml). 
Also the cloning happens only if the target folder does not exist yet (no automatic pull).

You can then include this package in your python packages list like this
`dppd=editable/code`. Anysnake2 will then (once) run a container tht 
runs `pip install -e .` on that (possibly cloned) folder, and include it
in the containers python path. It will also, on every run, parse the
requirements.txt/setup.cfg of the package and add it's requirements to
the python packages to resolve & install.

# Rebuilding

Rebuilding happens automatically whenever 

* the flake.nix changes
* flake/result/rootfs does not exist
* a previous build did not finish


# Container runtime

Anysnake2 uses [singularity](https://singularity.hpcng.org/) as a container runtime,
since it offers rootless containers that can run from locally
unpacked images (running from an image file unfortunately requires root and 
the +s binary singularity usess for that is not available using nix).

The actual run command is printed out on every run, and also stored in 'flake/run_scripts/<cmd>/singularity.bash'.

You can influence the mounted volumes using `[container.volumes_ro]` for read only and `[container.volumes_rw`] for read/write
volumes. Environment variables can be set using the `[container.env]` section.

Network is shared with the host, so have your firewalls up folks.

The actual commands can be surrounded by pre/post run commands outside/inside the container - see
the [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml). 

Build in commands (which you can not replace by config) are 

 * `attach` attach to still running container (interactive if more than 1 present)
 * * `build rootfs` - just build the (unpacked) container as a symlink tree
 * `build sif` - build the container image in flake/result/anysnake2_container.sif
 * `config` - list the available example configs (and config <name> to print one)
 * `develop` - run 'nix develop' on the flake and come back to flake/../
 * `help` - help
 * `version` - output anysnake2 version
 * `run --` - run arbitrary commands (without pre/post wrappers). Everything after -- is passed on to the container


# Singularity won't run

Singularity needs some folders that can not be created by Nix.

Outside of NixOS, you'll need to create them: `sudo mkdir -p /var/singularity/mnt/{container,final,overlay,session}`.

If you're using NixOS, setting 'programs.singularity.enable = true' should install them
(and a singularity installation we won't necessarily be using, we use the version in the outside_nixpkgs defined by anysnake2.toml instead)).



# Version policy

Anysnake2 will follow semver once 1.0 is reached.
But with the auto-use-the-specified-version-mechanism, it's a bit of a moot point.


# Installation

Either add this repository as a flake to your nix configuration,
or download one of the prebuild binaries (which are statically linked against musl) and place it somewhere in your $PATH.


# Proxy support
anysnake2 respects HTTPS_PROXY and HTTP_PROXY environment variables.


# Dtach

Singularity containers, unlike the daemon spawend docker containers of yore,
die if you were using them via ssh and disconnect.

That's no way to live. Anysnake2 therefore starts your containers in a ['dtach'](https://github.com/crigler/dtach),
a lightweight screen alternative.

You can manually detach by pressing 'ctrl+\'.

To reattach after a disconnect, use `anysnake2 attach`.

You can disable dtach by setting `container.detach = false` in your projects anysnake2.toml


# Why are path:/ urls on flakes not allowed

I've found nix flakes to mishandle 'path:/<absolute_path>?rev=xyz' style input urls.
As in it wouldn't actually checkout xyz, but push the whole repo including .git into the 
store. Then if you changed the repo in any way, it would fail with a narHash mismatch.

The workaround is to use just an /absolute_path instead, for "/absolut_path?rev=xyz" 
is being handled correctly.

To avoid you falling into this trap, anysnake2 requests path:// flake definitions.



