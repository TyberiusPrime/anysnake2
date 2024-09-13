## Anysnake2

Fully reproducible C/Python/R/Rust environments for research.

Anysnake2 levers [Nix](https://nixos.org/), [mach-nix](https://github.com/DavHau/mach-nix), 
[nixR](https://github.com/TyberiusPrime/nixR) and [rust-overlay](https://github.com/oxalica/rust-overlay)
to give you 'virtual environments' that are fully defined in an easy to use [toml](https://github.com/toml-lang/toml) file.

# How it works

The first thing the anysnake2 does is read the anysnake2 version from your project config file.
It then restarts itself with that exact anysnake2 version using Nix (see below).

Next it writes a [Nix flake](https://nixos.wiki/wiki/Flakes), and turns it into a 'symlink forest' which works
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

Using just a fixed revision of nixpkgs (the community recipe collection) in a flake, you already have a reproducible environment.

But it is limited to the versions of everything present in that nixpkgs
revision. Anysnake2 integrates other projects to introduce flexibility.

Extending nix (which does not do 'version resolution'), anysnake2 uses the concept of a 'date-locked ecosystem'
 to get a reproducible set of versions from a given set of requirements, essentially producing 'reproducible lock files'.

For R this is done using the [nixR](https://github.com/TyberiusPrime/nixR) project, which offers two or more ecosystem dates per bioconductor release.

For python, anysnake2 uses the [ancient-poetry](https://codeberg.com/TyberiusPrime/ancient_poetry) for the version resolution
in conjunction with [poetry2nix](https://github.com/nix-community/poetry2nix/).

Note that while the defaults are limited to your ecosystem date, you can always override individual packages with exact (and newer!) versions.

# Prerequisites

You need a working nix installation with flakes.

If you're using NixOS, referer to the [nix wiki](https://nixos.wiki/wiki/Flakes#NixOS)
Otherwise, you could use the [nix-unstable installer](https://github.com/numtide/nix-unstable-installer).

The following examples use nix to temporarily install anysnake2 (until your next nix-collect-garbage),
see the [installation section](# Installation)

# Getting started.

Run `nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 config basic >anysnake2.toml` and
have a look at the resulting toml file, which contains all the basic configuration (use `config full` for an example
containing every option. anysnake2.toml is the default config filename).

You should find something close to this.

```toml
# basic anysnake2.toml example
# package settings
[anysnake2]
    use_binary=false # optional, default = true. Download anysnake2 binary instead of building from source (both via a flake)


[nixpkgs]
# the nixpkgs used inside the container
        packages = [ # use https://search.nixos.org/packages to search
        "fish",
]
        url = "github:NixOS/nixpkgs/master/24.05"


[python] # python section is optional
        ecosystem_date="2021-08-16" # you get whatever packages the solver would have produced on that day
        version="3.9" # does not go down to 3.8.x. That's implicit in the nixpkgs (for now)


[python.packages]
# you can use version specifiers from https://www.python.org/dev/peps/pep-0440/#id53
        notebook=""
        pandas="1.2.0"


[rust]
        url = "github:oxalica/rust-overlay/master/d720bf3cebac38c2426d77ee2e59943012854cb8"
        version="1.55.0"


[container.env]
        ANYSNAKE2="1"


# container settings
[container.volumes_rw]
        "." = "/project" # map the current folder to /project


[cmd.default]
        run = """
cd /project
jupyter notebook
"""


[cmd.shell]
        run = """fish
"""


[cmd.test_pre_post]
        post_run_outside = """
echo "post_run"
"""
        pre_run_outside = """
echo "pre_run"
"""
        run = """
echo "run"
"""
        while_run_outside ="""
while :
do
        # write pid to pre_run.txt
        echo "$BASHPID" > while_run.txt
        sleep 1;
done
"""


[outside_nixpkgs]
# the nixpkgs used to run singularity and nixfmt
        url = "github:NixOS/nixpkgs/master/24.05"


[ancient_poetry]
        url = "git+https://codeberg.org/TyberiusPrime/ancient-poetry.git?ref=main&rev=54a06abec3273f42f9d86a36f184dbb3089cd9c9"


[poetry2nix]
        url = "github:nix-community/poetry2nix/master/cc0af1948e0887cd280496bd891fd40e52b40ff4"


[flake-util]
        url = "github:numtide/flake-utils/main/b1d9ab70662946ef0850d488da1c9019f3a9752a"


```

This defines an anysnake2 project that uses nixpkgs 24.05, both inside the container, and outside 
(which defines the singularity version and auxiliaries used), python from 2021-04-12, and neither R nor Rust.

If you now run `nix shell "github:TyberiusPrime/anysnake2" -c anysnake2`,
you'll get a jupyter notebook running after some downloading (thanks to the 'default' command defined in anysnake2.toml). 
But the next time will be very quick. (See the "singularity won't run" section below if
singularity complains about `failed to resolve session directory
/var/singularity/mnt/session`).

Note that by default, singularity maps your home into the container. You can adjust that with the container.home setting
(see [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml)).

For example, try adding 'pandas>=1.2' to the list of python packages above, and restart the nix shell command. 
You'll find that pandas 1.2.4 has been installed. That's not the most recent release - but it was at the python
ecosystem date we've been using. Change that to just a day later, and rerun `nix shell ...`,  and you'll get 
pandas 1.3.0 instead.

(A quick way to check the pandas version is
`nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 run -- python -c "'import pandas; print(pandas.__version__)'"`.
Yes the escaping between nix shell, anysnake and python in series is a bit of a mess.)

Instead of the default command (which is defined by cmd.default in the config toml) you can also run a custom command with an arbitrary
name (minus some build-in-exclusions), like the `shell` command defined above.
`nix shell "github:TyberiusPrime/anysnake2" -c anysnake2 shell`, will execute a fish shell inside your container.

# Using R

Including R and R packages using
[nixR](https://github.com/TyberiusPrime/nixR) is even
simpler than python packages, since you will get whatever R, bioconductor and
package version was current at a particular ecosystem date.

Just include this:

```toml
[R] # R section is optional
	date = "2024-05-10"
# the date definies the R version, bioconductor version and R packages you get.
	packages = [ "ACA", "Rcpp" ]

```

(Visit [nixR date overview](https://github.com/TyberiusPrime/nixR/blob/main/generated/readme.md) too see available dates.)


# Using Rust
```toml
[rust] # rust section is optional
version = "1.55.0" # leave off for 'newest', see tofu 
```

# Other flakes
Include other flakes like this.

```toml
[flakes.hello]
	url = "github:/TyberiusPrime/hello_flake" #https://nixos.wiki/wiki/Flakes#Input_schema - relative paths are tricky
	rev = "f32e7e451e9463667f6a1ddb7a662ec70d35144b" # flakes.lock tends to update unexpectedly, so we tie it down here
	follows = ["nixpkgs"] # so we overwrite the flakes dependencies
	packages = ["defaultPackage.x86_64-linux"] # it defaults to defualtPackage.x86_64-linux if you leave off packages. Use packages = [] to not include any packages
```

# The Tofu (trust-on-first-use) mechanism and anysnake2.toml rewriting

You can essentially start with an *empty* anysnake2.toml, and 
everything missing will be filled in by the anysnake2 itself.

That means defaulting to the newest versions and dates for ecosystem.

E.g. adding just `[rust]` will lead to the newest stable rust version, 
and adding `scanpy = "pypi"` to python.packages will lead to the newest scapny version (independend of ecosystem date!.
The newest-within-ecosystem date version from pypi would just be `scanpy = ""`).

Similar for all the places you can use 'urls' like 'github:/TyberiusPrime/dppd/master/<hash>', if you leave of the hash,
the newest commit in that branch will be used. And if you leave of the branch, master/main will be autodetected.

Everything that's tofued in this way is written down in anysnake2.toml, locking it in place.

There's also auto-formatting and pretty printing in place (down to the *order* of entries in anysnake2.toml), 
so anysnake2.tomls always look uniform.


# Clones 

Anysnake2 can be used to clone repositories you want to work on into this project folder.

```toml
[clones.code] # target directory
# separate from python packages so you can clone other stuff as well
ancient_poetry="git+https://codeberg.com/TyberiusPrime/ancient_poetry"
```

# Editable python installs

To work on python packages, you can use editable installs.

These always start with a repo url to clone, and are then installed twice.
Once in nix, producing all the necessary inputs for your package,
and then in editable mode from a local clone.

This looks like this:

```toml
[python.packages]
	dppd = {editable = true, url= "github:TyberiusPrime/dppd/master/d16b71a43b731fcf0c0e7e1c50dfcc80d997b7d7", poetry2nix.nativeBuildInputs=['setuptools']}
    ```


# Poetry2nix escape hatches.

While we have the poetry2nix override collection active, you'll need to help it out with build-systems
and possibly build tweaks on occasion.

```
[pytohn.packages]
    session-info = {poetry2nix.nativeBuildInputs=['setuptools']}
```

Refer to the 
[full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml)).
for more such tweaks.



# Jupyter


If you want jupyterlab, add `jupyterlab=""`.

If you want pip installable dependencies, like [jupyter-black](pypi.org/project/jupyter-black/) or [jupyterlab_code_formatter](https://jupyterlab-code-formatter.readthedocs.io/en/latest/installation.html#installation-step-1-installing-the-plugin-itself), add them the same way. The later will need both [black](https://pypi.org/project/black/) and [isort](https://pycqa.github.io/isort/) installed as well.

If you want an R kernel, add `IRkernel` to your `[R]/packages` list. 

If you want
[EvCxR](https://github.com/google/evcxr/blob/main/evcxr_jupyter/README.md) for
a rust kernel, add 'evcxr' to your `[nixpkgs]/packages`.

For both R and EvCxR, anysnake2 will automatically detect their presence and
copy the kernelspec to 'the right place'.

For other kernels, you'll need to figure out how to dump the kernel spec into
rootfs/usr/share/jupyter/kernels, patch flake_writer.rs and submit a PR.

## jupyterWith
Why are we not using [jupyterWith][https://github.com/tweag/jupyterWith]? Well, three reasons: 
 
* it's currently [not wrapping jupyter-notebook](https://github.com/tweag/jupyterWith/pull/142),
* it throws in another couple of python environments into the mix.
* I failed in convincing it to actually install a jupyter extension. It tries funky stuff with jupyter-labs extension manager,
  when all I needed was to add a couple of pip installable packages.



# Rebuilding

Rebuilding happens automatically whenever 

* the flake.nix changes
* or flake/result/rootfs does not exist
* or a previous build did not finish

# The auto-use-the-specified-version-mechanism

If anysnake2 find's it's own version to differ from the one defined in the config file,
it will restart itself using `nix shell github:.../`.

By default it will use the [TyberiusPrime/anysnake2_release_flakes](https://github.com/TyberiusPrime/anysnake2_release_flakes) repository, which 
provides download flakes for the releases from [TyberiusPrime/anysnake2](https://github.com/TyberiusPrime/anysnake2),
but if you specify 'use_binary=false' in the anysnake2 section, it will rebuild instead. 

Either way you can replace the repository the anysnake2 pulls itself from,
see the [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml)).




# Container runtime

Anysnake2 uses [singularity](https://singularity.hpcng.org/) as a container runtime,
since it offers rootless containers that can run from locally
unpacked images (running from an image file unfortunately requires root and
the +s binary singularity uses for that is not available using nix (on non NixOS systems).

The actual run command is printed out on every run, and also stored in 'flake/run_scripts/<cmd>/singularity.bash'.

You can influence the mounted volumes using `[container.volumes_ro]` for read only and `[container.volumes_rw`] for read/write
volumes. Environment variables can be set using the `[container.env]` section.

Network is shared with the host, so have your firewalls up folks.

The actual commands can be surrounded by pre/post run commands outside/inside the container - see
the [full example](https://github.com/TyberiusPrime/anysnake2/blob/main/examples/full/anysnake2.toml). 

Build in commands (which you can not replace by config) are 

 * `attach` attach to still running container (interactive if more than 1 present)
 * `build rootfs` - just build the (unpacked) container as a symlink tree
 * `build oci` - build a container image. See the section on OCI images
 * `build flake` - just write the flake to .anysnake2_flake/flake.nix
 * `config` - list the available example configurations (use config <name> to print one)
 * `develop` - run 'nix develop' on the flake and come back to flake/../ (shell can be configured via `[devShell]/shell`)
 * `help` - help
 * `version` - output anysnake2 version
 * `run --` - run arbitrary commands (without pre/post wrappers). Everything after -- is passed on to the container

# OCI images
Anysnake2 can build standards compliant OCI images using "build oci".

You can run it e.g. with podman: `podman run -it oci-archive:.anysnake2_flake/result bash`



# FAQ

## Why containers?

Because our use case profits from the mount namespace isolation, and we want
to create images to run on HPC (high performance computing) clusters.

You can always use `anysnake2 develop` to run outside of a container.

## Singularity won't run

Singularity needs some folders that can not be created by Nix.

Outside of NixOS, you'll need to create them: `sudo mkdir -p /var/singularity/mnt/{container,final,overlay,session}`.

If you're using NixOS, setting 'programs.singularity.enable = true' should install them
(and a singularity installation we won't necessarily be using, we use the version in the outside_nixpkgs defined by anysnake2.toml instead)).


## Version policy

Anysnake2 follows semver. Versions are major.minor.patch ,
with patch only fixing bugs, minor introducing features, and major being breaking changes
that you need to rewrite your anysnake2.toml for.

## Installation

Either add this repository as a flake to your nix configuration,
or download one of the prebuild binaries (which are statically linked against musl) and place it somewhere in your $PATH.

## Proxy support
anysnake2 respects HTTPS_PROXY and HTTP_PROXY environment variables.


## Dtach

Singularity containers, unlike the daemon spawend docker containers of yore,
die if you were using them via ssh and disconnect.

That's no way to live. Anysnake2 therefore starts your containers in a ['dtach'](https://github.com/crigler/dtach),
a lightweight screen alternative.

You can manually detach by pressing 'ctrl+\'.

To reattach after a disconnect, use `anysnake2 attach` from your project folder.
If there are multiple running containers, you will be asked which one you want to reattach. 

You can disable dtach by setting `container.detach = false` in your projects anysnake2.toml.
dtach is also disable if you're running in screen or tmux (if $STY or $TMUX are set).


## Why are path:/ urls on flakes not allowed?

I've found nix flakes to mishandle 'path:/<absolute_path>?rev=xyz' style input urls.

As in it would not actually checkout xyz, but push the whole repo including .git into the 
store. Then if you changed the repo in any way, it would fail with a narHash mismatch.

The workaround is to use just an /absolute_path instead, for "/absolut_path?rev=xyz" 
is being handled correctly.

To avoid you falling into this trap, anysnake2 rejects path:// flake definitions.


## Exit Codes

Anysnake2 strives to follow the ['sysexit' codes](https://www.freebsd.org/cgi/man.cgi?query=sysexits), 
that means that the default 'an error occured' exit code is 70.
65 means the configuration toml couldn't be understood, 66 it's missing.


## Why singularity?

We want a container engine that
  
 * runs from images in the file system 
 * works without root
 * doesn't containerize the network (at least optionally)

Singularity fits the bill.

I suppose we could also have gone with runc and a lot of manual handholding.


## GitHub API limits 

Anysnake2 uses the github api, for example to  translate python-ecosystem-dates into the correct
python-deps-db commit. Though these are cached, you may run into Github's ratelimit 
on these requests (especially since the API can only retrieve a 100 commits at once, 
and the limit is around 60 requests/hour).

Anysnake2 can use a personal-access token for the GitHub API. Just create one in Github/Settings/develop settings,
without any permissions, and supply it and your username via the env variables ANYSNAKE2_GITHUB_API_USERNAME and
ANYSNAKE2_GITHUB_API_PASSWORD.



