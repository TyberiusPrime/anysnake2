extern crate clap;
use anyhow::{anyhow, bail, Context, Result};
use clap::{Arg, ArgMatches};
use config::{BuildPythonPackageInfo, PythonPackageDefinition};
use ex::fs;
use indoc::indoc;
use lazy_static::lazy_static;
use log::{debug, error, info, trace, warn};
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use std::io::BufRead;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{borrow::Cow, ffi::OsStr};
use std::{collections::HashMap, str::FromStr};
use toml_edit::{value, Document, Table};
use url::Url;

/* TODO

* R/r_ecosystem_track

* pypyi-debs that were not flakes... when is the cut off , how do we get around it 2021-04-12, is
  it even worth it?

* Per command volumes? Do we need these?

* Establish a test matrix

* Ensure that the singularity sif container  actually contains everything...
*
*
* * test hg?rev=xyz clone
* * test wrong urls (no git+, etc)

*/

mod config;
mod flake_writer;
mod maps_duplicate_key_is_error;
mod python_parsing;

use flake_writer::lookup_github_tag;

const VERSION: &str = env!("CARGO_PKG_VERSION");

trait CloneStringLossy {
    fn to_string_lossy(&self) -> String;
}

impl CloneStringLossy for PathBuf {
    fn to_string_lossy(&self) -> String {
        self.clone().into_os_string().to_string_lossy().to_string()
    }
}
impl CloneStringLossy for Path {
    fn to_string_lossy(&self) -> String {
        self.to_owned()
            .into_os_string()
            .to_string_lossy()
            .to_string()
    }
}
/*
impl CloneStringLossy for std::ffi::OsStr {
    fn to_string_lossy(&self) -> String {
        self.to_owned()
            .to_string_lossy()
            .to_string()
    }
}
*/
#[derive(Debug)]
pub struct ErrorWithExitCode {
    msg: String,
    exit_code: i32,
}

impl ErrorWithExitCode {
    fn new(exit_code: i32, msg: String) -> Self {
        ErrorWithExitCode { msg, exit_code }
    }
}

impl std::fmt::Display for ErrorWithExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

fn main() {
    let r = inner_main();
    match r {
        Err(e) => {
            error!("{:?}", e); //so the error messages are colorfull
            let code = match e.downcast_ref::<ErrorWithExitCode>() {
                Some(ewe) => ewe.exit_code,
                None => 70,
            };
            std::process::exit(code);
        }
        Ok(_) => {
            std::process::exit(0);
        }
    }
}

lazy_static! {
    static ref CTRL_C_ALLOWED: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
}

fn install_ctrl_c_handler() -> Result<()> {
    let c = CTRL_C_ALLOWED.clone();
    Ok(ctrlc::set_handler(move || {
        if c.load(Ordering::Relaxed) {
            println!("anysnake aborted");
            std::process::exit(1);
        }
    })?)
}

fn parse_args() -> ArgMatches {
    clap::Command::new("Anysnake2")
        .version(VERSION)
        .author("Florian Finkernagel <finkernagel@imt.uni-marburg.de>")
        .about("Sane version declaration and container generation using nix")
        .allow_external_subcommands(true)
        .arg(
            Arg::new("no-version-switch")
                .long("no-version-switch")
                .help("do not change to toml file defined version")
                .action(clap::ArgAction::SetTrue)
            )
        .arg(
            Arg::new("config_file")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Sets a custom config file")
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .value_name("LEVEL")
                //.default_value("2")
                .help("Sets the level of verbosity (0=quiet,1=error/warnings, 2=info (default), 3=debug, 4=trace, 5=trace)"),
        )
        .arg(
            Arg::new("_running_version")
                .long("_running_version")
                .help("internal use only")
                .hide(true)
                .action(clap::ArgAction::Set)
        )
        .subcommand(
            clap::Command::new("build").about("build containers (see subcommands), but do not run anything")
            .subcommand(
                clap::Command::new("flake").about("write just the flake, but don't nix build anything"),
            )
            .subcommand(
                clap::Command::new("rootfs").about("build rootfs container (used for singularity)"),
            )
            .subcommand(
                clap::Command::new("sif").about("build SIF (singularity) container image (anysnake2_container.sif)"),
            )

        )
        .subcommand(
            clap::Command::new("config")
                .about("dump different example anysnake2.toml to stdout")
                .subcommand(clap::Command::new("basic"))
                .subcommand(clap::Command::new("minimal"))
                .subcommand(clap::Command::new("full"))
        )
        .subcommand(clap::Command::new("develop").about("run nix develop, and go back to this dir with your favourite shell"))
        .subcommand(clap::Command::new("version").about("the version actually used by the config file. Error if no config file is present (use --version for the version of this binary"))
        .subcommand(clap::Command::new("attach").about("attach to previously running session"))

        .subcommand(
            clap::Command::new("upgrade")
            .arg(
                Arg::new("what").num_args(1..).action(clap::ArgAction::Append), //.last(true), // Indicates that `slop` is only accessible after `--`.
                ).about("query remotes and upgrade anysnake2.toml accordingly")
        )
        .subcommand(
            clap::Command::new("run")
                .about("run arbitray commands in container (w/o any pre/post bash scripts)")
                .arg(
                    Arg::new("slop").num_args(1..).action( clap::ArgAction::Append) , //.last(true), // Indicates that `slop` is only accessible after `--`.
                ),
        )
        .arg(
            Arg::new("slop").num_args(1..).action( clap::ArgAction::Append,) //.last(true), // Indicates that `slop` is only accessible after `--`.
        ) //todo: argument passing to the scripts? 
        .get_matches()
}

fn handle_config_command(matches: &ArgMatches) -> Result<bool> {
    if let Some(("config", sc)) = matches.subcommand() {
        match sc.subcommand() {
            Some(("minimal", _)) => println!(
                "{}",
                std::include_str!("../examples/minimal/anysnake2.toml")
            ),
            Some(("full", _)) => {
                println!("{}", std::include_str!("../examples/full/anysnake2.toml"))
            }
            Some(("basic", _)) => {
                // includes basic
                println!("{}", std::include_str!("../examples/basic/anysnake2.toml"))
            }
            _ => {
                bail!("Could not find that config. Try to pass minimial/basic/full as in  'anysnake2 config basic'");
            }
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

fn configure_logging(matches: &ArgMatches) -> Result<()> {
    let default_verbosity = 2;
    let str_verbosity = matches.get_one::<String>("verbose");
    let verbosity: usize =  match str_verbosity {
        Some(str_verbosity) => usize::from_str(&str_verbosity).context("Failed to parse verbosity. Must be an integer")?,
        None => default_verbosity
    };
    stderrlog::new()
        .module(module_path!())
        .quiet(verbosity == 0)
        .verbosity(verbosity)
        .show_level(false)
        .timestamp(stderrlog::Timestamp::Off)
        .init()
        .unwrap();

    if verbosity > default_verbosity {
        info!("verbosity set to {}", verbosity);
    }
    Ok(())
}

fn switch_to_configured_version(
    parsed_config: &config::MinimalConfigToml,
    matches: &ArgMatches,
    flake_dir: impl AsRef<Path>,
) -> Result<()> {
    if parsed_config.anysnake2.rev == "dev" {
        info!("Using development version of anysnake");
    } else if matches.contains_id("no-version-switch") {
        info!("--no-version-switch was passed, not switching versions");
    } else if parsed_config.anysnake2.rev
        != *matches
            .get_one::<String>("_running_version")
            .cloned()
            .unwrap_or_else(|| "noversionspecified".to_string())
    {
        info!("restarting with version {}", &parsed_config.anysnake2.rev);
        let repo = format!(
            "{}?rev={}",
            &parsed_config.anysnake2.url.as_ref().unwrap(),
            lookup_github_tag(
                parsed_config.anysnake2.url.as_ref().unwrap(),
                &parsed_config.anysnake2.rev,
                flake_dir
            )?
        );

        let mut args = vec![
            "shell",
            &repo,
            "-c",
            "anysnake2",
            "--_running_version",
            &parsed_config.anysnake2.rev,
        ];
        let input_args: Vec<String> = std::env::args().collect();
        {
            for argument in input_args.iter().skip(1) {
                args.push(argument);
            }
            trace!("new args {:?}", args);
            debug!("running nix {}", &args.join(" "));
            let status = run_without_ctrl_c(|| Ok(Command::new("nix").args(&args).status()?))?;
            //now push
            std::process::exit(status.code().unwrap());
        }
    }
    Ok(())
}

struct CollectedPythonPackages {
    requirement_packages: Vec<(String, String)>,
    build_packages: HashMap<String, BuildPythonPackageInfo>,
}

fn collect_python_packages(
    parsed_config: &mut config::ConfigToml,
) -> Result<CollectedPythonPackages> {
    Ok(match &mut parsed_config.python {
        Some(python) => {
            let mut requirement_packages: Vec<(String, String)> = Vec::new();
            let mut build_packages: HashMap<String, BuildPythonPackageInfo> = HashMap::new();
            for (name, pp) in python.packages.drain() {
                match pp {
                    PythonPackageDefinition::Requirement(str_package_definition) => {
                        requirement_packages.push((name, str_package_definition));
                    }
                    PythonPackageDefinition::BuildPythonPackage(bp_definition) => {
                        build_packages.insert(name, bp_definition);
                    }
                }
            }
            debug!("found python packages {:?}", &requirement_packages);
            if !requirement_packages.is_empty() {
                //don't need pip if we ain't got no packages (and therefore no editable packages
                requirement_packages.push(("pip".into(), "".into())); // we use pip to build editable packages
                requirement_packages.push(("setuptools".into(), "".into())); // we use pip to build editable packages

                let editable_paths: Vec<String> = requirement_packages
                    .iter()
                    .filter_map(|(pkg, spec)| {
                        spec.strip_prefix("editable/")
                            .map(|editable_path| editable_path.to_string() + "/" + pkg)
                    })
                    .collect();
                debug!("found editable_paths: {:?}", &editable_paths);

                let python_requirements_from_editable =
                    python_parsing::find_python_requirements_for_editable(&editable_paths)?;
                for (pkg, version_spec) in python_requirements_from_editable.into_iter() {
                    requirement_packages.push((pkg, version_spec));
                }
            }
            CollectedPythonPackages {
                requirement_packages,
                build_packages,
            }
        }
        None => CollectedPythonPackages {
            requirement_packages: Vec::new(),
            build_packages: HashMap::new(),
        },
    })
}

#[allow(clippy::vec_init_then_push)]
fn inner_main() -> Result<()> {
    install_ctrl_c_handler()?;
    let matches = parse_args();
    configure_logging(&matches)?;

    if handle_config_command(&matches)? {
        return Ok(());
    };

    let top_level_slop: Vec<String> = match matches.get_many::<String>("slop") {
        Some(slop) => slop.cloned().collect(),
        None => Vec::new(),
    };

    let cmd = match matches.subcommand() {
        Some((name, _subcommand)) => name,
        _ => {
            if top_level_slop.is_empty() {
                "default"
            } else {
                &top_level_slop[0]
            }
        }
    };

    if std::env::var("SINGULARITY_NAME").is_ok() {
        bail!("Can't run anysnake within singularity container - nesting not supported");
    }

    let config_file = matches
        .get_one::<String>("config_file")
        .cloned()
        .unwrap_or_else(|| "anysnake2.toml".to_string());
    if cmd == "version" && !Path::new(&config_file).exists() {
        //output the version of binary
        print_version_and_exit();
    }

    let flake_dir: PathBuf = [".anysnake2_flake"].iter().collect();
    fs::create_dir_all(&flake_dir)?; //we must create it now, so that we can store the anysnake tag lookup

    let minimal_parsed_config: config::MinimalConfigToml =
        config::MinimalConfigToml::from_file(&config_file)?;
    if cmd != "upgrade" {
        //otherwise you could never upgrade < 1.10 versions
        switch_to_configured_version(&minimal_parsed_config, &matches, &flake_dir)?;
    }

    let mut parsed_config: config::ConfigToml = config::ConfigToml::from_file(&config_file)?;
    if cmd == "version" {
        //output the version you'd actually be using!
        print_version_and_exit();
    }

    if cmd == "attach" {
        let outside_nixpkgs_url = format!(
            "{}?rev={}",
            &parsed_config.outside_nixpkgs.url,
            lookup_github_tag(
                &parsed_config.outside_nixpkgs.url,
                &parsed_config.outside_nixpkgs.rev,
                &flake_dir
            )?,
        );

        return attach_to_previous_container(&flake_dir, &outside_nixpkgs_url);
    }

    let use_generated_file_instead = parsed_config.anysnake2.do_not_modify_flake.unwrap_or(false);

    if cmd == "upgrade" {
        return upgrade(
            matches
                .subcommand()
                .unwrap()
                .1
                .get_many::<String>("what")
                .map(|x| x.cloned().collect()),
            &parsed_config,
            use_generated_file_instead,
        );
    }

    if !(parsed_config.cmd.contains_key(cmd) || cmd == "build" || cmd == "run" || cmd == "develop")
    {
        bail!(
            "Cmd {} not found.
            Available from config file: {:?}
            Available from anysnake2: build, run, example-config, version
            ",
            cmd,
            parsed_config.cmd.keys()
        );
    }

    lookup_missing_flake_revs(&mut parsed_config)?;

    lookup_clones(&mut parsed_config)?;
    perform_clones(&parsed_config)?;

    let temp = collect_python_packages(&mut parsed_config)?;
    let (python_packages, mut python_build_packages) =
        (temp.requirement_packages, temp.build_packages);
    trace!(
        "python packages: {:?} {:?}",
        python_packages,
        python_build_packages
    );

    let nixpkgs_url = format!(
        "{}?rev={}",
        &parsed_config.outside_nixpkgs.url,
        lookup_github_tag(
            &parsed_config.outside_nixpkgs.url,
            &parsed_config.outside_nixpkgs.rev,
            &flake_dir
        )?,
    );
    apply_trust_on_first_use(&mut parsed_config, &mut python_build_packages, &nixpkgs_url)?;

    let flake_changed = flake_writer::write_flake(
        &flake_dir,
        &mut parsed_config,
        &python_packages,
        &python_build_packages,
        use_generated_file_instead,
    )?;

    if let Some(("build", sc)) = matches.subcommand() {
        {
            match sc.subcommand() {
                Some(("flake", _)) => {
                    println!("Writing just flake/flake.nix");
                    rebuild_flake(use_generated_file_instead, "flake", &flake_dir)?;
                }
                Some(("sif", _)) => {
                    println!("Building sif in flake/result/...sif");
                    rebuild_flake(
                        use_generated_file_instead,
                        "sif_image.x86_64-linux",
                        &flake_dir,
                    )?;
                }
                Some(("rootfs", _)) => {
                    println!("Building rootfs in flake/result");
                    rebuild_flake(use_generated_file_instead, "", &flake_dir)?;
                }
                _ => {
                    println!("Please pass a subcommand as to what to build");
                    std::process::exit(1);
                }
            }
        }
    } else {
        let run_dir: PathBuf = flake_dir.join("run_scripts").join(cmd);
        fs::create_dir_all(&run_dir)?;
        let run_sh: PathBuf = run_dir.join("run.sh");
        let run_sh_str: String = run_sh.into_os_string().to_string_lossy().to_string();
        fs::write(
            &run_sh_str,
            format!(
                "#/bin/bash\ncd ..&& echo 'starting nix develop shell'\n {}\n",
                &parsed_config.dev_shell.shell
            ),
        )
        .context("Failed to write run.sh")?; // the -i makes it read /etc/bashrc

        let build_output: PathBuf = flake_dir.join("result/rootfs");
        let build_unfinished_file = flake_dir.join(".build_unfinished"); // ie. the flake build failed
                                                                         //
                                                                         //early error exit if you try to run an non-existant command
        if flake_changed || !build_output.exists() || build_unfinished_file.exists() {
            info!("Rebuilding flake");
            rebuild_flake(use_generated_file_instead, "", &flake_dir)?;
        }

        if let Some(python) = &parsed_config.python {
            fill_venv(&python.version, &python_packages, &nixpkgs_url, &flake_dir)?;
        };

        if cmd == "develop" {
            if let Some(python) = &parsed_config.python {
                write_develop_python_path(&flake_dir, &python_packages, &python.version)?;
            }
            run_without_ctrl_c(|| {
                let s = format!("../{}", &run_sh_str);
                let full_args = vec!["develop", "-c", "bash", &s];
                println!("{:?}", full_args);
                Ok(Command::new("nix")
                    .current_dir(&flake_dir)
                    .args(full_args)
                    .status()?)
            })?;
        } else {
            let home_dir = PathBuf::from(replace_env_vars(
                parsed_config.container.home.as_deref().unwrap_or("$HOME"),
            ));
            let home_dir_str: String = fs::canonicalize(&home_dir)
                .context("home dir not found")?
                .into_os_string()
                .to_string_lossy()
                .to_string();
            debug!("Using {:?} as home", home_dir);
            fs::create_dir_all(home_dir).context("Failed to create home dir")?;

            let outer_run_sh: PathBuf = run_dir.join("outer_run.sh");
            let run_sh: PathBuf = run_dir.join("run.sh");
            fs::create_dir_all(&run_dir).context("Failed to create run dir for scripts")?;
            let post_run_sh: PathBuf = run_dir.join("post_run.sh");
            let mut post_run_outside: Option<String> = None;
            let mut parallel_running_child: Option<std::process::Child> = None;

            if cmd == "run" {
                let slop = matches.subcommand().unwrap().1.get_many::<String>("slop");
                let slop: Vec<String> = match slop {
                    Some(slop) => slop.cloned().collect(),
                    None => {
                        bail!("ad hoc command (=run) passed, but nothing to actually run passed")
                    }
                };
                if slop.is_empty() {
                    bail!("no command passed after run");
                }
                info!("Running singularity with ad hoc - cmd {:?}", slop);
                fs::write(&outer_run_sh, "#/bin/bash\nbash -i /anysnake2/run.sh\n")?; // the -i makes it read /etc/bashrc
                fs::write(&run_sh, slop.join(" "))?;
                fs::write(&post_run_sh, "")?;
            } else {
                let cmd_info = parsed_config.cmd.get(cmd).context("Command not found")?;
                match &cmd_info.pre_run_outside {
                    Some(bash_script) => {
                        info!("Running pre_run_outside for cmd - cmd {}", cmd);
                        run_bash(bash_script).with_context(|| format!("pre run outside failed. Script:\n{}", add_line_numbers(bash_script)))?;
                    }
                    None => {}
                };
                if let Some(while_run_outside) =  &cmd_info.while_run_outside { 
                    parallel_running_child = Some(spawn_bash(while_run_outside)?);


                }
                info!("Running singularity - cmd {}", cmd);
                let run_template = std::include_str!("run.sh");
                let run_script = run_template.replace("%RUN%", &cmd_info.run);
                let post_run_script = run_template
                    .replace("%RUN%", cmd_info.post_run_inside.as_deref().unwrap_or(""));
                fs::write(
                &outer_run_sh,
                "#/bin/bash\nbash -i /anysnake2/run.sh $@\nexport ANYSNAKE_RUN_STATUS=$?\nbash /anysnake2/post_run.sh", //the -i makes it read /etc/bashrc
            )?;
                fs::write(&run_sh, run_script)?;
                fs::write(&post_run_sh, post_run_script)?;
                post_run_outside = cmd_info.post_run_outside.clone();
            }

            let outer_run_sh_str: String =
                outer_run_sh.into_os_string().to_string_lossy().to_string();
            let run_sh_str: String = run_sh.into_os_string().to_string_lossy().to_string();
            let post_run_sh_str: String =
                post_run_sh.into_os_string().to_string_lossy().to_string();

            let mut singularity_args: Vec<String> = vec![
                "exec".into(),
                "--userns".into(),
                "--cleanenv".into(),
                "--home".into(),
                home_dir_str,
            ];
            let mut binds = Vec::new();
            binds.push((
                "/nix/store".to_string(),
                "/nix/store".to_string(),
                "ro".to_string(),
            ));
            let mut envs = Vec::new();
            let mut paths = vec!["/bin"];
            binds.push((
                run_sh_str,
                "/anysnake2/run.sh".to_string(),
                "ro".to_string(),
            ));
            binds.push((
                post_run_sh_str,
                "/anysnake2/post_run.sh".to_string(),
                "ro".to_string(),
            ));
            binds.push((
                outer_run_sh_str,
                "/anysnake2/outer_run.sh".to_string(),
                "ro".to_string(),
            ));
            if let Some(python) = parsed_config.python {
                let venv_dir: PathBuf = flake_dir.join("venv").join(&python.version);
                error!("{:?}", venv_dir);
                binds.push((
                    venv_dir.to_string_lossy(),
                    "/anysnake2/venv".to_string(),
                    "ro".to_string(),
                ));
                let mut python_paths = Vec::new();
                for (pkg, spec) in python_packages
                    .iter()
                    .filter(|(_, spec)| spec.starts_with("editable/"))
                {
                    let safe_pkg = safe_python_package_name(pkg);
                    let target_dir: PathBuf = [spec.strip_prefix("editable/").unwrap(), pkg]
                        .iter()
                        .collect();
                    binds.push((
                        target_dir.to_string_lossy(),
                        format!("/anysnake2/venv/linked_in/{}", safe_pkg),
                        "ro".to_string(),
                    ));
                    let egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
                    let egg_target = parse_egg(egg_link)?;
                    python_paths.push(egg_target)
                }
                envs.push(format!("PYTHONPATH={}", python_paths.join(":")));
                paths.push("/anysnake2/venv/bin");
            };

            match &parsed_config.container.volumes_ro {
                Some(volumes_ro) => {
                    for (from, to) in volumes_ro {
                        let from: PathBuf = fs::canonicalize(from)
                            .context(format!("canonicalize path failed on {} (read only volume - does the path exist?)", &from))?;
                        let from = from.into_os_string().to_string_lossy().to_string();
                        binds.push((from, to.to_string(), "ro".to_string()));
                    }
                }
                None => {}
            };
            match &parsed_config.container.volumes_rw {
                Some(volumes_ro) => {
                    for (from, to) in volumes_ro {
                        let from: PathBuf = fs::canonicalize(from)
                            .context(format!("canonicalize path failed on {} (read/write volume - does the path exist?)", &from))?;
                        let from = from.into_os_string().to_string_lossy().to_string();
                        binds.push((from, to.to_string(), "rw".to_string()));
                    }
                }
                None => {}
            }
            for (from, to, opts) in binds {
                singularity_args.push("--bind".into());
                singularity_args.push(format!(
                    "{}:{}:{}",
                    //fs::canonicalize(from)?
                    //.into_os_string()
                    //.to_string_lossy(),
                    from,
                    to,
                    opts
                ));
            }

            if let Some(container_envs) = &parsed_config.container.env {
                for (k, v) in container_envs.iter() {
                    envs.push(format!("{}={}", k, replace_env_vars(v)));
                }
            }

            envs.push(format!("PATH={}", paths.join(":")));

            for e in envs.into_iter() {
                singularity_args.push("--env".into());
                singularity_args.push(e);
            }

            singularity_args.push(flake_dir.join("result/rootfs").to_string_lossy());
            singularity_args.push("/bin/bash".into());
            singularity_args.push("/anysnake2/outer_run.sh".into());
            for s in top_level_slop.iter().skip(1) {
                singularity_args.push(s.to_string());
            }
            let dtach_socket = match &parsed_config.anysnake2.dtach {
                true => {
                    if std::env::var("STY").is_err() && std::env::var("TMUX").is_err() {
                        Some(format!(
                            "{}_{}",
                            cmd,
                            chrono::Local::now().format("%Y-%m-%d_%H:%M:%S")
                        ))
                    } else {
                        None
                    }
                }
                false => None,
            };

            let singularity_result = run_singularity(
                &singularity_args[..],
                &nixpkgs_url,
                Some(&run_dir.join("singularity.bash")),
                dtach_socket,
                &flake_dir,
            )?;
            if let Some(bash_script) = post_run_outside {
                if let Err(e) = run_bash(&bash_script) {
                    warn!(
                        "An error occured when running the post_run_outside bash script: {}\nScript: {}",
                        e, 
                        add_line_numbers(&bash_script)
                    )
                }
            };
            if let Some(mut parallel_running_child) = parallel_running_child {
                parallel_running_child.kill().context("Failed to kill parallel running child")?;
            }
            std::process::exit(
                singularity_result
                    .code()
                    .context("No exit code inside container?")?,
            );
        }
    }
    Ok(())
}

fn run_without_ctrl_c<T>(func: impl Fn() -> Result<T>) -> Result<T> {
    CTRL_C_ALLOWED.store(false, Ordering::SeqCst);
    let res = func();
    CTRL_C_ALLOWED.store(true, Ordering::SeqCst);
    res
}

fn run_singularity(
    args: &[String],
    outside_nix_repo: &str,
    log_file: Option<&PathBuf>,
    dtach_socket: Option<String>,
    flake_dir: &Path,
) -> Result<std::process::ExitStatus> {
    let singularity_url = format!("{}#singularity", outside_nix_repo);
    register_nix_gc_root(&singularity_url, flake_dir)?;
    run_without_ctrl_c(|| {
        let mut nix_full_args: Vec<String> = Vec::new();
        let using_dtach = if let Some(dtach_socket) = &dtach_socket {
            let dtach_dir = flake_dir.join("dtach");
            fs::create_dir_all(dtach_dir)?;
            let dtach_url = singularity_url.replace("#singularity", "#dtach");
            register_nix_gc_root(&dtach_url, flake_dir)?;
            nix_full_args.extend(vec![
                //vec just to shut up clippy
                "shell".to_string(),
                dtach_url,
                "-c".to_string(),
                "dtach".to_string(),
                "-c".to_string(), // create a new session
                flake_dir.join("dtach").join(dtach_socket).to_string_lossy(),
                "nix".to_string(),
            ]);
            true
        } else {
            false
        };

        nix_full_args.extend(vec![
            //vec just to shutup clippy
            "shell".to_string(),
            singularity_url.clone(),
            "-c".into(),
            "singularity".into(),
        ]);
        for arg in args {
            nix_full_args.push(arg.to_string());
        }
        let pp = pretty_print_singularity_call(&nix_full_args);
        if let Some(lf) = log_file {
            let o = format!("nix {}", pp.trim_start());
            fs::write(lf, o)?;
        }
        info!("nix {}", pp.trim_start());
        if using_dtach {
            // dtach eats the current screen
            // so we want to push enough newlines to preserve our output
            use terminal_size::{terminal_size, Height, Width};
            let empty_lines = match terminal_size() {
                Some((Width(_w), Height(h))) => h,
                None => 50,
            };
            for _ in 0..empty_lines {
                println!();
            }
        }
        std::io::stdout().flush()?;

        Ok(Command::new("nix").args(nix_full_args).status()?)
    })
}

fn print_version_and_exit() -> ! {
    println!("anysnake2 version: {}", VERSION);
    std::process::exit(0);
}

fn pretty_print_singularity_call(args: &[String]) -> String {
    let mut res = "".to_string();
    let mut skip_space = false;
    for arg in args.iter() {
        if skip_space {
            skip_space = false
        } else {
            res += "    ";
        }
        if arg.contains('&') {
            res += "\"";
            res += arg;
            res += "\"";
        } else {
            res += arg;
        }
        if !(arg == "--bind" || arg == "--env" || arg == "--home" || arg == "singularity") {
            res += " \\\n";
        } else {
            skip_space = true;
            res += " ";
        }
    }
    res.pop();
    res += "\n";
    res
}

/// expand clones by clone_regular_expressions, verify url schema

fn lookup_clones(parsed_config: &mut config::ConfigToml) -> Result<()> {
    let clone_regexps: Vec<(Regex, &String)> = match &parsed_config.clone_regexps {
        Some(replacements) => {
            let mut res = Vec::new();
            for (from, to) in replacements {
                let r = Regex::new(&format!("^{}$", from))
                    .context(format!("failed to parse {}", from))?;
                res.push((r, to))
            }
            res
        }
        None => Vec::new(),
    };
    match &mut parsed_config.clones {
        Some(clones) => {
            for (_target_dir, name_urls) in clones.iter_mut() {
                for (name, proto_url) in name_urls.iter_mut() {
                    for (re, replacement) in &clone_regexps {
                        if re.is_match(proto_url) {
                            let mut out = proto_url.to_string();
                            for group in re.captures_iter(proto_url) {
                                //there only ever is one
                                out = replacement.replace("\\0", name);
                                for ii in 1..group.len() {
                                    out = out.replace(&format!("\\{}", ii), &group[ii]);
                                }
                                //println_f!("match {name}={url} {re} => {out}");
                            }
                            *proto_url = out; // know it's the real url
                        }
                    }
                    if !(proto_url.starts_with("git+")
                        || proto_url.starts_with("hg+")
                        || proto_url.starts_with("file://"))
                    {
                        bail!("Url did not start with git+, hg+ or file:// which are the only supported version control formats {}. (Possibly rewritten using clone_regexps", proto_url);
                    }
                }
            }
        }
        None => {}
    };
    //assert!(re.is_match("2014-01-01"));

    Ok(())
}

fn dir_empty(path: &Path) -> Result<bool> {
    Ok(path
        .read_dir()
        .context("Failed to read_dir")?
        .next()
        .is_none())
}

fn perform_clones(parsed_config: &config::ConfigToml) -> Result<()> {
    match &parsed_config.clones {
        Some(clones) => {
            for (target_dir, name_urls) in clones.iter() {
                fs::create_dir_all(target_dir)
                    .context(format!("Could not create {}", target_dir))?;
                let clone_log: PathBuf = [target_dir, ".clone_info.json"].iter().collect();
                let mut known_clones: HashMap<String, String> = match clone_log.exists() {
                    true => serde_json::from_str(&fs::read_to_string(&clone_log)?)?,
                    false => HashMap::new(),
                };
                for (name, url) in name_urls {
                    let known_url = known_clones.get(name).map(String::as_str).unwrap_or("");
                    let final_dir: PathBuf = [target_dir, name].iter().collect();
                    if known_url != url && final_dir.exists() && !dir_empty(&final_dir)?
                    //empty dir is ok.
                    {
                        let msg = format!(
                            "Url changed for clone target: {target_dir}/{name}. Was '{known_url}' is now '{url}'.\n\
                        Cowardly refusing to throw away old checkout."
                        , target_dir=target_dir, name=name, known_url=known_url, url=url);
                        bail!(msg);
                    }
                }
                for (name, url) in name_urls {
                    let final_dir: PathBuf = [target_dir, name].iter().collect();
                    fs::create_dir_all(&final_dir)?;
                    if dir_empty(&final_dir)? {
                        info!("cloning {}/{} from {}", target_dir, name, url);
                        known_clones.insert(name.clone(), url.clone());
                        let (cmd, trunc_url) = if url.starts_with("git+") {
                            ("git", url.strip_prefix("git+").unwrap())
                        } else if url.starts_with("hg+") {
                            ("hg", url.strip_prefix("hg+").unwrap())
                        } else if url.starts_with("file://") {
                            ("cp", &url[..])
                        } else {
                            bail!("Unexpected url schema - should have been tested before (bug in anysnake. try git+https)");
                        };
                        let parsed_url =
                            Url::parse(trunc_url) //I can't change the scheme from git+https to https, with this libary
                                .with_context(|| {
                                    format!("Failed to parse {} as an url", &trunc_url)
                                })?;

                        let mut base_url = parsed_url.clone();
                        base_url.set_query(None);
                        let clone_url_for_cmd = base_url.as_str();
                        let clone_url_for_cmd = match clone_url_for_cmd.strip_prefix("file://") {
                            Some(path) => {
                                if path.starts_with('/') {
                                    path.to_string()
                                } else {
                                    // we are in target/package, so we need to go up to to make it
                                    // relative again
                                    "../../".to_string() + (path.strip_prefix("./").unwrap_or(path))
                                }
                            }
                            None => clone_url_for_cmd.to_string(),
                        };
                        let output = run_without_ctrl_c(|| match cmd {
                            "hg" | "git" => Command::new(cmd)
                                .args(["clone", &clone_url_for_cmd, "."])
                                .current_dir(&final_dir)
                                .output()
                                .context(format!(
                                    "Failed to execute clone {target_dir}/{name} from {url} .",
                                    target_dir = target_dir,
                                    name = name,
                                    url = url
                                )),
                            "cp" => {
                                let args = [
                                    "-c",
                                    &format!(
                                        "cp {}/* . -a",
                                        &clone_url_for_cmd
                                            .strip_suffix('/')
                                            .unwrap_or(&clone_url_for_cmd)
                                    )[..],
                                ];
                                dbg!(&args);
                                Command::new("bash")
                                    .args(args)
                                    .current_dir(&final_dir)
                                    .output()
                                    .context(format!(
                                        "Failed to execute copy {target_dir}/{name} from {url} .",
                                        target_dir = target_dir,
                                        name = name,
                                        url = url
                                    ))
                            }

                            _ => Err(anyhow!("Unsupported clone cmd?!")),
                        })?;
                        if !output.status.success() {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let msg = format!(
                                "Failed to clone {target_dir}/{name} from {url}. \n Stdout {stdout:?}\nStderr: {stderr:?}",
                            target_dir = target_dir, name = name, url = url, stdout=stdout, stderr=stderr);
                            bail!(msg);
                        }

                        for (k, v) in parsed_url.query_pairs() {
                            let v = v.to_string();
                            if k == "rev" {
                                let args: Vec<&str> = if cmd == "git" {
                                    ["checkout", &v].into()
                                } else if cmd == "hg" {
                                    ["checkout", "-r", &v].into()
                                } else {
                                    bail!("Should not be reached");
                                };
                                let output = run_without_ctrl_c(|| {
                                    Command::new(cmd)
                                        .args(&args)
                                        .current_dir(&final_dir)
                                        .output()
                                        .context(format!(
                                            "Failed to execute checkout revision {v} in {target_dir}",
                                            v=v,
                                            target_dir = target_dir,
                                        ))
                                })?;
                                if !output.status.success() {
                                    let stdout = String::from_utf8_lossy(&output.stdout);
                                    let stderr = String::from_utf8_lossy(&output.stderr);
                                    let msg = format!(
                                        "Failed to checkout {v} in {target_dir}. \n Stdout {stdout:?}\nStderr: {stderr:?}",
                                        target_dir = target_dir, v = v, stdout=stdout, stderr=stderr);
                                    bail!(msg);
                                }
                            } else {
                                bail!(
                                    "Could not understand url for {target_dir}: {url}",
                                    target_dir = &target_dir,
                                    url = &url
                                );
                            }
                        }
                    }
                }
                fs::write(
                    &clone_log,
                    serde_json::to_string_pretty(&json!(known_clones))?,
                )
                .with_context(|| format!("Failed to write {:?}", &clone_log))?;
            }
        }
        None => {}
    };

    Ok(())
}

fn rebuild_flake(
    use_generated_file_instead: bool,
    target: &str,
    flake_dir: impl AsRef<Path>,
) -> Result<()> {
    debug!("writing flake");

    if !use_generated_file_instead {
        run_without_ctrl_c(|| {
            Command::new("git")
                .args(["commit", "-m", "autocommit"])
                .current_dir(&flake_dir)
                .output()
                .context("Failed git add flake.nix")
        })?;
    }
    let build_unfinished_file = flake_dir.as_ref().join(".build_unfinished");
    fs::write(&build_unfinished_file, "in_progress")?;

    if target != "flake" {
        debug!("building container");
        let nix_build_result = run_without_ctrl_c(|| {
            Command::new("nix")
                .args(["build", &format!("./#{}", target), "-v",
                "--max-jobs", "auto",
                "--cores", "4",
                "--keep-going"
                ]
                )
                .current_dir(&flake_dir)
                .status()
                .with_context(|| format!("nix build failed. Perhaps try with --show-trace using 'nix build ./#{} -v --show-trace'",
                    target))
        })?;
        if nix_build_result.success() {
            fs::remove_file(&build_unfinished_file)?;
            Ok(())
        } else {
            Err(anyhow!("flake building failed"))
        }
    } else {
        Ok(())
    }
}

fn spawn_bash(script: &str) -> Result<std::process::Child> {
    let mut child = Command::new("bash").stdin(Stdio::piped()).spawn()?;
    let child_stdin = child.stdin.as_mut().unwrap();
    child_stdin.write_all(b"set -euo pipefail\n")?;
    child_stdin.write_all(script.as_bytes())?;
    child_stdin.write_all(b"\n")?;
    Ok(child)

}

fn run_bash(script: &str) -> Result<()> {
    run_without_ctrl_c(|| {
        let mut child = spawn_bash(script)?;
        let ecode = child.wait().context("Failed to wait on bash")?; // closes stdin
        if ecode.success() {
            Ok(())
        } else {
            Err(anyhow!("Bash error return code {}", ecode))
        }
    })
}

fn replace_env_vars(input: &str) -> String {
    let mut output = input.to_string();
    for (k, v) in std::env::vars() {
        output = output.replace(&format!("${}", k), &v);
        output = output.replace(&format!("${{{}}}", k), &v);
    }
    output
}

fn safe_python_package_name(input: &str) -> String {
    input.replace('_', "-")
}

fn fill_venv(
    python_version: &str,
    python: &[(String, String)],
    outside_nixpkgs_url: &str, //clones: &HashMap<String, HashMap<String, String>>, //target_dir, name, url
    flake_dir: &Path,
) -> Result<()> {
    let venv_dir: PathBuf = flake_dir.join("venv").join(python_version);
    fs::create_dir_all(&venv_dir.join("bin"))?;
    fs::create_dir_all(flake_dir.join("venv_develop"))?;
    let mut to_build = Vec::new();

    let target_python: PathBuf = PathBuf::from_str(".anysnake2_flake/result/rootfs/bin/python")
        .unwrap()
        .canonicalize()
        .context("failed to find python binary in container")?;
    let target_python_str = target_python.to_string_lossy();

    for (pkg, spec) in python
        .iter()
        .filter(|(_, spec)| spec.starts_with("editable/"))
    {
        let safe_pkg = safe_python_package_name(pkg);
        let target_dir: PathBuf = [spec.strip_prefix("editable/").unwrap(), pkg]
            .iter()
            .collect();
        if !target_dir.exists() {
            bail!("editable python package that was not present in file system (missing clone)? looking for package {} in {:?}",
                               pkg, target_dir);
        }
        let egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
        let venv_used = {
            let anysnake_link = venv_dir.join(format!("{}.anysnake-link", safe_pkg));
            if !anysnake_link.exists() {
                "".to_string()
            } else {
                ex::fs::read_to_string(anysnake_link)?
            }
        };
        if !egg_link.exists() || venv_used != target_python_str {
            // so that changing python versions triggers a rebuild.
            to_build.push((safe_pkg, target_dir));
        }
    }
    if !to_build.is_empty() {
        for (safe_pkg, target_dir) in to_build.iter() {
            info!("Pip install {:?}", &target_dir);
            let td = tempfile::Builder::new().prefix("anysnake_venv").tempdir()?; // temp /tmp
            let td_home = tempfile::Builder::new().prefix("anysnake_venv").tempdir()?; // temp home directory
            let td_home_str = td_home.path().to_string_lossy().to_string();

            let search_python = extract_python_exec_from_python_env_bin(&target_python)?;
            println!("target_python {:?}", target_python);
            println!("search_python {:?}", search_python);

            let mut cmd_file = tempfile::NamedTempFile::new()?;
            writeln!(
                cmd_file,
                indoc!(
                    "
                set -eux pipefail
                cat /anysnake2/install.sh
                mkdir /tmp/venv
                cd /anysnake2/venv/linked_in/{safe_pkg} && \
                    pip \
                    --disable-pip-version-check \
                    install \
                    --no-deps \
                    -e . \
                    --prefix=/tmp/venv \
                    --ignore-installed
                $(python <<EOT
                from pathlib import Path
                for fn in Path('/tmp/venv/bin').glob('*'):
                    input = fn.read_text()
                    if '{search_python}' in input:
                        output = input.replace('{search_python}', '{target_python}')
                        fn.write_text(output)
                EOT
                )
                cp /tmp/venv/bin/* /anysnake2/venv/bin 2>/dev/null|| true
               "
                ),
                safe_pkg = &safe_pkg,
                search_python = search_python,
                target_python = &target_python_str,
            )
            .context("failed to write tmp file with cmd")?;

            let mut singularity_args: Vec<String> = vec![
                "exec".into(),
                "--userns".into(),
                "--cleanenv".into(),
                //"--no-home".into(),
                "--home".into(),
                td_home_str,
                "--bind".into(),
                "/nix/store:/nix/store:ro".into(),
                "--bind".into(),
                format!("{}:/tmp:rw", &td.path().to_string_lossy()),
                "--bind".into(),
                format!(
                    "{}:/anysnake2/venv:rw",
                    venv_dir.clone().into_os_string().to_string_lossy()
                ),
                "--bind".into(),
                format!(
                    "{}:/anysnake2/venv/linked_in/{}:rw",
                    target_dir.clone().into_os_string().to_string_lossy(),
                    &safe_pkg
                ),
                "--bind".into(),
                format!(
                    "{}:/anysnake2/venv/bin:rw",
                    venv_dir.join("bin").into_os_string().to_string_lossy()
                ),
                "--bind".into(),
                format!(
                    "{}:/anysnake2/install.sh",
                    cmd_file.path().to_string_lossy()
                ),
            ];
            singularity_args.push(flake_dir.join("result/rootfs").to_string_lossy());
            singularity_args.push("bash".into());
            singularity_args.push("/anysnake2/install.sh".into());
            info!("installing inside singularity");
            let singularity_result = run_singularity(
                &singularity_args[..],
                outside_nixpkgs_url,
                Some(&venv_dir.join("singularity.bash")),
                None,
                flake_dir,
            )
            .context("singularity failed")?;
            if !singularity_result.success() {
                bail!(
                    "Singularity pip install failed with exit code {}",
                    singularity_result.code().unwrap()
                );
            }
            // now copy the egg/pth files..
            // appearntly bin patching is no longer necessary.
            let source_egg_folder = td
                .path()
                .join("venv/lib")
                .join(format!("python{}", python_version))
                .join("site-packages");
            let target_egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
            let paths = fs::read_dir(&source_egg_folder)
                .context("could not read site-packages folder in temp venv")?;
            let mut any_found = false;
            for path in paths {
                let path = path.unwrap().path();
                let suffix = path.extension().unwrap_or(OsStr::new("")).to_string_lossy();
                if suffix == "pth" || suffix == "egg-link" {
                    //we want to read {safe_pkg}.egg-link, not __editable__{safe_pkg}-{version}.pth
                    //because we don't *know* the version
                    //and this happens only once
                    fs::write(
                        target_egg_link,
                        ex::fs::read_to_string(path).context("Failed reading source  link")?,
                    )?;
                    any_found = true;
                    break;
                }
            }
            if !any_found {
                let paths = fs::read_dir(source_egg_folder).unwrap();
                for path in paths {
                    let path = path.unwrap().path();
                    info!("found in venv folder {}", path.display());
                }
                bail!("Could not find .egg or .pth in venv folder");
            }

            let target_anysnake_link = venv_dir.join(format!("{}.anysnake-link", safe_pkg));
            fs::write(target_anysnake_link, &target_python_str)
                .context("target anysnake link write failed")?;

            /*keep it here in case we need it again...
             * for dir_entry in walkdir::WalkDir::new(td.path()) {
                let dir_entry = dir_entry?;
                if let Some(filename) = dir_entry.file_name().to_str() {
                    if filename.ends_with(".egg-link") {
                        trace!("found {:?} for {:?}", &safe_pkg, &dir_entry);
                        fs::write(
                            target_egg_link,
                            fs::read_to_string(dir_entry.path())?,
                        )?;
                        break;
                    }
                };
            }
            */
        }
    }
    Ok(())
}

fn extract_python_exec_from_python_env_bin(path: &PathBuf) -> Result<String> {
    let text: Vec<u8> =
        std::fs::read(path).with_context(|| format!("failed reading {:?}", path))?;
    let binary_re = regex::bytes::Regex::new("'NIX_PYTHONEXECUTABLE' '([^']+)'").unwrap();
    let hits = binary_re.captures(&text);
    let out = match hits {
        Some(x) => std::str::from_utf8(&x[1])?.to_string(),
        None => {
            let text = std::str::from_utf8(&text).with_context(|| {
                format!(
                    "failed utf-8 converting {:?}, but also had no NIX_PYTHONEXECUTABLE",
                    path
                )
            })?;
            let re = Regex::new("exec \"([^\"]+)\"").unwrap();
            let out: String = re
                .captures_iter(text)
                .next()
                .context(format!("Could not find exec in {:?}", &path))?[1]
                .to_string();
            out
        }
    };

    Ok(out)
}

#[allow(non_snake_case)]
#[derive(Deserialize, Debug)]
struct NixFlakePrefetchOutput {
    storePath: String,
}

#[derive(Deserialize, Debug)]
struct NixBuildOutputs {
    out: String,
}

#[derive(Deserialize, Debug)]
struct NixBuildOutput {
    outputs: NixBuildOutputs,
}

fn prefetch_flake(url_without_hash: &str) -> Result<String> {
    debug!("nix prefetching flake {}", &url_without_hash);
    run_without_ctrl_c(|| {
        let output = std::process::Command::new("nix")
            .args(["flake", "prefetch", url_without_hash, "--json"])
            .output()?;
        if !output.status.success() {
            Err(anyhow!("nix build failed"))
        } else {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let j: NixFlakePrefetchOutput = serde_json::from_str(&stdout)?;
            //now from the gc_dir
            Ok(j.storePath)
        }
    })
}

fn register_gc_root(store_path: &str, symlink: &Path) -> Result<()> {
    debug!("registering gc root for {} at {:?}", &store_path, &symlink);
    run_without_ctrl_c(|| {
        let output = std::process::Command::new("nix-store")
            .args([
                "--realise",
                store_path,
                "--add-root",
                &symlink.to_string_lossy(),
            ])
            .output()?;
        if !output.status.success() {
            Err(anyhow!("nix-store realise failed"))
        } else {
            Ok(())
        }
    })
}

fn nix_build_flake(url: &str) -> Result<String> {
    run_without_ctrl_c(|| {
        let output = std::process::Command::new("nix")
            .args([
                "build",
                url,
                "--max-jobs",
                "auto",
                "--cores",
                "4",
                "--no-link",
                "--json",
            ])
            .output()?;
        if !output.status.success() {
            Err(anyhow!("nix build failed"))
        } else {
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("{}", stdout);
            let j: Vec<NixBuildOutput> = serde_json::from_str(&stdout)?;
            let j = j.into_iter().next().unwrap();
            Ok(j.outputs.out)
        }
    })
}

pub fn register_nix_gc_root(url: &str, flake_dir: impl AsRef<Path>) -> Result<()> {
    debug!("registering gc root for {}", url);
    //where we store this stuff
    let gc_roots = flake_dir.as_ref().join(".gcroots");
    fs::create_dir_all(&gc_roots)?;

    let (without_hash, _) = url.rsplit_once('#').expect("GC_root url should contain #");
    //first we store and hash the flake itself and record tha.
    let flake_symlink_here = gc_roots.join(without_hash.replace('/', "_"));
    if !flake_symlink_here.exists() {
        let store_path = prefetch_flake(without_hash)?;
        register_gc_root(&store_path, &flake_symlink_here)?;
    }

    let build_symlink_here = gc_roots.join(url.replace('/', "_"));
    if !build_symlink_here.exists() {
        let store_path = nix_build_flake(url)?;
        register_gc_root(&store_path, &build_symlink_here)?;
    }
    Ok(())
}

fn attach_to_previous_container(flake_dir: impl AsRef<Path>, outside_nix_repo: &str) -> Result<()> {
    let mut available: Vec<_> = fs::read_dir(flake_dir.as_ref().join("dtach"))
        .context("Could not find dtach socket directory")?
        .filter_map(|x| x.ok())
        .collect();
    if available.is_empty() {
        bail!("No session to attach to available");
    } else if available.len() == 1 {
        println!("reattaching to {:?}", available[0].file_name());
        run_dtach(available[0].path(), outside_nix_repo)
    } else {
        available.sort_unstable_by_key(|x| x.file_name());
        loop {
            println!("please choose an entry to reattach (number+enter), or ctrl-c to abort");
            for (ii, entry) in available.iter().enumerate() {
                println!("\t{} {:?}", ii, entry.file_name());
            }
            let line1 = std::io::stdin().lock().lines().next().unwrap().unwrap();
            for (ii, entry) in available.iter().enumerate() {
                if format!("{}", ii) == line1 {
                    return run_dtach(entry.path(), outside_nix_repo);
                }
            }
            println!("sorry I did not understand that. \n")
        }
    }
}

fn upgrade(
    what: Option<Vec<String>>,
    parsed_config: &config::ConfigToml,
    use_generated_file_instead: bool,
) -> Result<()> {
    match what {
        None => {
            println!("no upgrade specified");
            println!("Available for upgrade");
            println!("\tanysnake2");
            return Ok(());
        }
        Some(what_) => {
            for w in what_ {
                if w == "anysnake2" {
                    println!("querying github for newest anysnake2 version");
                    let repo = parsed_config
                        .anysnake2
                        .url
                        .as_ref()
                        .context("no anysnake2 url???")?
                        .strip_prefix("github:");
                    match repo {
                        Some(repo) => {
                            let gh_tags = flake_writer::get_github_tags(repo, 1)?;
                            if !gh_tags.is_empty() {
                                let newest =
                                    gh_tags.first().expect("No tags though vec was not empty!?");
                                let newest_tag = newest["name"]
                                    .as_str()
                                    .context("Could not find name for tag in github api output")?
                                    .to_string();
                                println!("found newest tag: {}", newest_tag);
                                println!("current tag: {}", parsed_config.anysnake2.rev);
                                if newest_tag != parsed_config.anysnake2.rev {
                                    if use_generated_file_instead {
                                        return Err(anyhow!(
                                            "do_not_modify_flake is set. Not upgrading anything"
                                        ));
                                    }
                                    if parsed_config.anysnake2.rev == "dev" {
                                        return Err(anyhow!("Currently the 'dev' version is specified. Not overwriting that."));
                                    }
                                    //todo: refactor into 'modify-toml-closure-taking-func'?
                                    let toml = std::fs::read_to_string(
                                        parsed_config.anysnake2_toml_path.as_ref().unwrap(),
                                    )
                                    .expect("Could not reread config file");
                                    let mut doc = toml.parse::<Document>().expect("invalid doc");
                                    doc["anysnake2"]["rev"] = value(newest_tag);
                                    let out_toml = doc.to_string();
                                    std::fs::write(
                                        parsed_config.anysnake2_toml_path.as_ref().unwrap(),
                                        out_toml,
                                    )
                                    .expect("failed to rewrite config file");
                                    println!("Upgraded anysnake2.toml");
                                } else {
                                    println!(
                                        "not upgrading anysnake2 entry - already at newest version",
                                    );
                                }
                            } else {
                                return Err(anyhow!("Could not find any version in {}", &repo));
                            }
                        }
                        None => {
                            return Err(anyhow!(
                                "Can only upgrade anysnake2 if it's being sourced from github"
                            ))
                        }
                    }
                } else {
                    return Err(anyhow!(
                        "Could not handle upgrade for {}, don't know what to do",
                        w
                    ));
                }
            }
        }
    };
    Ok(())
}

fn run_dtach(p: impl AsRef<Path>, outside_nix_repo: &str) -> Result<()> {
    let dtach_url = format!("{}#dtach", outside_nix_repo);
    let nix_full_args = vec![
        "shell".to_string(),
        dtach_url,
        "-c".to_string(),
        "dtach".to_string(),
        "-a".to_string(),
        p.as_ref().to_owned().to_string_lossy(),
    ];
    let status = Command::new("nix").args(nix_full_args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("dtach reattachment failed"))
    }
}
fn parse_egg(egg_link: impl AsRef<Path>) -> Result<String> {
    let raw = fs::read_to_string(egg_link)?;
    Ok(match raw.split_once('\n') {
        Some(x) => x.0.to_string(),
        None => raw,
    })
}

fn write_develop_python_path(
    flake_dir: impl AsRef<Path>,
    python_packages: &[(String, String)],
    python_version: &str,
) -> Result<()> {
    let mut develop_python_paths = Vec::new();
    let venv_dir: PathBuf = flake_dir.as_ref().join("venv").join(python_version);
    let parent_dir: PathBuf = fs::canonicalize(&flake_dir)?
        .parent()
        .context("No parent found for flake dir")?
        .to_path_buf();

    for (pkg, spec) in python_packages
        .iter()
        .filter(|(_, spec)| spec.starts_with("editable/"))
    {
        let safe_pkg = safe_python_package_name(pkg);
        let real_target = parent_dir.join(spec.strip_prefix("editable/").unwrap());
        let egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
        let egg_target = parse_egg(egg_link)?;
        let egg_target =
            egg_target.replace("/anysnake2/venv/linked_in", &real_target.to_string_lossy());

        develop_python_paths.push(egg_target)
    }
    fs::write(
        flake_dir.as_ref().join("develop_python_path.bash"),
        format!("export PYTHONPATH=\"{}\"", &develop_python_paths.join(":")),
    )?;
    Ok(())
}

enum PrefetchHashResult {
    Hash(String),
    HaveToUseFetchGit,
}

type Updates = Vec<(Vec<String>, toml_edit::Value)>;

/// if no hash_{rev} is set, discover it and update anysnake2.toml
/// if no rev is set, discover it as well
fn apply_trust_on_first_use(
    //todo: Where ist the flake stuff?
    config: &mut config::ConfigToml,
    python_build_packages: &mut HashMap<String, BuildPythonPackageInfo>,
    outside_nixpkgs_url: &str,
) -> Result<()> {
    let mut updates: Updates = Vec::new();
    apply_trust_on_first_use_python(
        config,
        python_build_packages,
        outside_nixpkgs_url,
        &mut updates,
    )?;
    apply_trust_on_first_use_r(config, &mut updates)?;

    if !updates.is_empty() {
        let toml = std::fs::read_to_string(config.anysnake2_toml_path.as_ref().unwrap())
            .expect("Could not reread config file");
        let mut doc = toml.parse::<Document>().expect("invalid doc");
        for (path, value) in updates {
            let mut x = &mut doc[&path[0]];
            for p in path[1..path.len() - 1].iter() {
                x = &mut x[p];
            }
            x[&path[path.len() - 1]] = toml_edit::Item::Value(value);
        }
        //now apply updates
        let out_toml = doc.to_string();
        std::fs::write(config.anysnake2_toml_path.as_ref().unwrap(), out_toml)
            .expect("failed to rewrite config file");
    }
    Ok(())
}

fn apply_trust_on_first_use_r(
    config: &mut config::ConfigToml,
    updates: &mut Updates,
) -> Result<()> {
    if let Some(r) = &mut config.r {
        if !r.packages.is_empty() {
            if let None = r.nixr_tag {
                println!(
                    "Using discover-newest on first use for nixR, updating your anysnake2.toml"
                );
                let rev = discover_newest_rev_git(&r.nixr_url, Some("main"))?;
                println!("\tDiscovered nixR revision {}", &rev);
                updates.push((
                    vec!["R".to_string(), "nixr_tag".to_string()],
                    rev.clone().into(),
                ));
                r.nixr_tag = Some(rev);
            }
        }
    }
    Ok(())
}

fn apply_trust_on_first_use_python(
    _config: &config::ConfigToml,
    python_build_packages: &mut HashMap<String, BuildPythonPackageInfo>,
    outside_nixpkgs_url: &str,
    updates: &mut Updates,
) -> Result<()> {
    if !python_build_packages.is_empty() {
        for (key, spec) in python_build_packages.iter_mut() {
            let method = spec
                .get("method")
                .expect("missing method - should have been caught earlier");
            let mut hash_key = "".to_string();
            match &method[..] {
                "fetchFromGitHub" => {
                    let rev = {
                        match spec.get("rev") {
                            Some(x) => x.to_string(),
                            None => {
                                println!("Using discover-newest on first use for python package {}, updating your anysnake2.toml", key);
                                let owner = spec.get("owner").expect("missing owner").to_string();
                                let repo = spec.get("repo").expect("missing repo").to_string();
                                let url = format!("https://github.com/{}/{}", owner, repo);
                                let rev = discover_newest_rev_git(
                                    &url,
                                    spec.get("branchName").map(AsRef::as_ref),
                                )?;
                                store_rev(spec, updates, key.to_owned(), &rev);
                                rev
                            }
                        }
                    };

                    hash_key = format!("hash_{}", rev);
                    if !spec.contains_key(&hash_key) {
                        println!("Using Trust-On-First-Use for python package {}, updating your anysnake2.toml", key);

                        let owner = spec.get("owner").expect("missing owner").to_string();
                        let repo = spec.get("repo").expect("missing repo").to_string();
                        let hash = prefetch_github_hash(&owner, &repo, &rev)?;
                        match hash {
                            PrefetchHashResult::Hash(hash) => {
                                println!("nix-prefetch-hash for {} is {}", key, hash);
                                store_hash(spec, updates, key.to_owned(), &hash_key, hash);
                            }
                            PrefetchHashResult::HaveToUseFetchGit => {
                                let fetchgit_url = format!("https://github.com/{}/{}", owner, repo);
                                let hash =
                                    prefetch_git_hash(&fetchgit_url, &rev, outside_nixpkgs_url)
                                        .context("prefetch-git-hash failed")?;

                                let mut out = Table::default();
                                out["method"] = value("fetchgit");
                                out["url"] =
                                    value(format!("https://github.com/{}/{}", owner, repo));
                                out["rev"] = value(&rev);
                                out[&hash_key] = value(&hash);
                                if spec.contains_key("branchName") {
                                    out["branchName"] = value(spec.get("branchName").unwrap());
                                }
                                updates.push((
                                    vec![
                                        "python".to_string(),
                                        "packages".to_string(),
                                        key.to_owned(),
                                    ],
                                    toml_edit::Value::InlineTable(out.into_inline_table()),
                                ));

                                spec.retain(|k, _| k == "branchName");
                                spec.insert("method".to_string(), "fetchgit".into());
                                spec.insert(hash_key.clone(), hash);
                                spec.insert("url".to_string(), fetchgit_url);
                                spec.insert("rev".to_string(), rev.clone());

                                println!("The github repo {}/{}/?rev={} is using .gitattributes and export-subst, which leads to the github tarball used by fetchFromGithub changing hashes over time.\nYour anysnake2.toml has been adjusted to use fetchgit instead, which is immune to that.", owner, repo, rev);
                            }
                        };
                    }
                }
                "fetchgit" => {
                    let url = spec
                        .get("url")
                        .expect("missing url on fetchgit")
                        .to_string();
                    let rev = {
                        match spec.get("rev") {
                            Some(x) => x.to_string(),
                            None => {
                                println!("Using discover-newest on first use for python package {}, updating your anysnake2.toml", key);
                                let rev = discover_newest_rev_git(
                                    &url,
                                    spec.get("branchName").map(AsRef::as_ref),
                                )?;
                                println!("\tDiscovered revision {}", &rev);
                                store_rev(spec, updates, key.to_owned(), &rev);
                                rev
                            }
                        }
                    };

                    hash_key = format!("hash_{}", rev);
                    if !spec.contains_key(&hash_key) {
                        println!("Using Trust-On-First-Use for python package {}, updating your anysnake2.toml", key);
                        let hash = prefetch_git_hash(&url, &rev, outside_nixpkgs_url)
                            .context("prefetch_git-hash failed")?;
                        store_hash(spec, updates, key.to_owned(), &hash_key, hash);
                        //bail!("bail1")
                    }
                }
                "fetchhg" => {
                    let url = spec.get("url").expect("missing url on fetchhg").to_string();
                    let rev = {
                        match spec.get("rev") {
                            Some(x) => x.to_string(),
                            None => {
                                println!("Using discover-newest on first use for python package {}, updating your anysnake2.toml", key);
                                let rev = discover_newest_rev_hg(&url)?;
                                println!("\tDiscovered revision {}", &rev);
                                store_rev(spec, updates, key.to_owned(), &rev);
                                rev
                            }
                        }
                    };
                    hash_key = format!("hash_{}", rev);
                    if !spec.contains_key(&hash_key) {
                        println!("Using Trust-On-First-Use for python package {}, updating your anysnake2.toml", key);
                        let hash = prefetch_hg_hash(&url, &rev, outside_nixpkgs_url).with_context(
                            || format!("prefetch_hg-hash failed for {} {}", url, rev),
                        )?;
                        store_hash(spec, updates, key.to_owned(), &hash_key, hash);
                        //bail!("bail1")
                    }
                }
                "useFlake" => {
                    // we use the flake rev, so no-op
                }

                "fetchPyPi" => {
                    return Err(anyhow!(
                        "fetchPyPi is not a valid method, you meant fetchPypi"
                    ));
                }

                "fetchPypi" => {
                    let pname = spec.get("pname").unwrap_or(key).to_string();
                    let version = match spec.get("version") {
                        Some(ver) => ver.to_string(),
                        None => {
                            println!("Retrieving current version for {} from pypi, updating your anysnake2.toml", key);
                            let version = get_newest_pipi_version(&pname)?;
                            store_version(spec, updates, key.to_owned(), &version);
                            println!("Received version {}", &version);
                            version
                        }
                    };

                    hash_key = format!("hash_{}", version);
                    if !spec.contains_key(&hash_key) {
                        println!("Using Trust-On-First-Use for python package {}, updating your anysnake2.toml", key);
                        let hash = prefetch_pypi_hash(&pname, &version, outside_nixpkgs_url)
                            .context("prefetch-pypi-hash")?;
                        store_hash(spec, updates, key.to_owned(), &hash_key, hash);
                        //bail!("bail1")
                    }
                }
                _ => {
                    println!("No trust-on-first-use for method {}, will likely fail with nix hash error!", &method);
                }
            };
            if !hash_key.is_empty() {
                spec.insert(
                    "sha256".to_string(),
                    spec.get(&hash_key.to_string()).unwrap().to_string(),
                );
                spec.retain(|key, _| !key.starts_with("hash_"));
            }
        }
    }
    Ok(())
}

/// helper for apply_trust_on_first_use
fn store_hash(
    spec: &mut BuildPythonPackageInfo,
    updates: &mut Updates,
    key: String,
    hash_key: &str,
    hash: String,
) {
    updates.push((
        vec![
            "python".to_string(),
            "packages".to_string(),
            key,
            hash_key.to_string(),
        ],
        hash.to_owned().into(),
    ));

    spec.insert(hash_key.to_string(), hash.to_owned());
}

/// helper for discover_rev_on_first_use
fn store_rev(
    spec: &mut BuildPythonPackageInfo,
    updates: &mut Updates,
    key: String, // teh package
    rev: &String,
) {
    updates.push((
        vec![
            "python".to_string(),
            "packages".to_string(),
            key,
            "rev".to_string(),
        ],
        rev.into(),
    ));
    spec.insert("rev".to_string(), rev.to_owned());
}

fn store_version(
    spec: &mut BuildPythonPackageInfo,
    updates: &mut Updates,
    key: String,
    version: &String,
) {
    updates.push((
        vec![
            "python".to_string(),
            "packages".to_string(),
            key,
            "version".to_string(),
        ],
        version.into(),
    ));
    spec.insert("version".to_string(), version.to_owned());
}

fn prefetch_git_hash(url: &str, rev: &str, outside_nixpkgs_url: &str) -> Result<String> {
    let nix_prefetch_git_url = format!("{}#nix-prefetch-git", outside_nixpkgs_url);
    let nix_prefetch_git_url_args = &[
        "shell",
        &nix_prefetch_git_url,
        "-c",
        "nix-prefetch-git",
        "--url",
        url,
        "--rev",
        rev,
        "--quiet",
    ];
    let stdout = Command::new("nix")
        .args(nix_prefetch_git_url_args)
        .output()
        .context("failed on nix-prefetch-git")?
        .stdout;
    let stdout = std::str::from_utf8(&stdout)?;
    let structured: HashMap<String, serde_json::Value> =
        serde_json::from_str(stdout).context("nix-prefetch-git output failed json parsing")?;
    let old_format = structured
        .get("sha256")
        .context("No sha256 in nix-prefetch-git json output")?;
    let old_format: &str = old_format.as_str().context("sha256 was no string")?;
    let new_format = convert_hash_to_subresource_format(old_format)?;

    Ok(new_format)
}

fn prefetch_hg_hash(url: &str, rev: &str, outside_nixpkgs_url: &str) -> Result<String> {
    let nix_prefetch_hg_url = format!("{}#nix-prefetch-hg", outside_nixpkgs_url);
    let nix_prefetch_hg_url_args = &[
        "shell",
        &nix_prefetch_hg_url,
        "-c",
        "nix-prefetch-hg",
        url,
        rev,
    ];
    let stdout = Command::new("nix")
        .args(nix_prefetch_hg_url_args)
        .output()
        .context("failed on nix-prefetch-hg")?
        .stdout;
    let stdout = std::str::from_utf8(&stdout)?.trim();
    let lines = stdout.split('\n');
    let old_format = lines
        .last()
        .expect("Could not parse nix-prefetch-hg output");
    let new_format = convert_hash_to_subresource_format(old_format)?;

    Ok(new_format)
}

fn prefetch_github_hash(owner: &str, repo: &str, git_hash: &str) -> Result<PrefetchHashResult> {
    let url = format!(
        "https://github.com/{owner}/{repo}/archive/{git_hash}.tar.gz",
        owner = owner,
        repo = repo,
        git_hash = git_hash
    );

    let stdout = Command::new("nix-prefetch-url")
        .args([&url, "--type", "sha256", "--unpack", "--print-path"])
        .output()
        .context(format!("Failed to nix-prefetch {url}", url = url))?
        .stdout;
    let stdout = std::str::from_utf8(&stdout)?;
    let mut stdout_split = stdout.split('\n');
    let old_format = stdout_split
        .next()
        .with_context(||format!("unexpected output from 'nix-prefetch-url {} --type sha256 --unpack --print-path' (line 0  - should have been hash)", url))?;
    let path = stdout_split
        .next()
        .with_context(||format!("unexpected output from 'nix-prefetch-url {} --type sha256 --unpack --print-path' (line 1  - should have been hash)", url))?;

    /* if the git repo is using .gitattributes and 'export-subst'
     * then github tarballs are actually not stable - if the drop out of the caching
     * expotr-subst might stamp a different timestamp into the substituted values
     * We detectd that and redirect to use fetchgit then
     */
    let gitattributes_path = PathBuf::from(path).join(".gitattributes");
    if gitattributes_path.exists() {
        let text = ex::fs::read_to_string(&gitattributes_path).context(format!(
            "failed to read .gitattributes from {:?}",
            &gitattributes_path
        ))?;
        if text.contains("export-subst") {
            return Ok(PrefetchHashResult::HaveToUseFetchGit);
        }
    }

    let new_format = convert_hash_to_subresource_format(old_format)?;
    println!("before convert: {}, after: {}", &old_format, &new_format);
    Ok(PrefetchHashResult::Hash(new_format))
}

fn get_newest_pipi_version(package_name: &str) -> Result<String> {
    use flake_writer::get_proxy_req; //todo: refactor out of flake_writer
    let json = get_proxy_req()?
        .get(&format!("https://pypi.org/pypi/{package_name}/json"))
        .call()?
        .into_string()?;
    let json: serde_json::Value = serde_json::from_str(&json)?;
    let version = json["info"]["version"]
        .as_str()
        .context("no version in json")?;
    Ok(version.to_string())
}

fn prefetch_pypi_hash(pname: &str, version: &str, outside_nixpkgs_url: &str) -> Result<String> {
    /*
         * nix-universal-prefetch pythonPackages.fetchPypi \
        --pname home-assistant-frontend \
        --version 20200519.1
    149v56q5anzdfxf0dw1h39vdmcigx732a7abqjfb0xny5484iq8w
    */
    let nix_prefetch_scripts = format!("{}#nix-universal-prefetch", outside_nixpkgs_url);
    let nix_prefetch_args = &[
        "shell",
        &nix_prefetch_scripts,
        "-c",
        "nix-universal-prefetch",
        "pythonPackages.fetchPypi",
        "--pname",
        pname,
        "--version",
        version,
    ];
    let stdout = Command::new("nix")
        .args(nix_prefetch_args)
        .output()
        .context("failed on nix-prefetch-url for pypi")?
        .stdout;
    let stdout = std::str::from_utf8(&stdout)?.trim();
    let lines = stdout.split('\n');
    let old_format = lines
        .last()
        .expect("Could not parse nix-prefetch-pypi output");
    let new_format = convert_hash_to_subresource_format(old_format)?;

    Ok(new_format)
}

fn convert_hash_to_subresource_format(hash: &str) -> Result<String> {
    if hash.is_empty() {
        return Err(anyhow!(
            "convert_hash_to_subresource_format called with empty hash"
        ));
    }
    let res = Command::new("nix")
        .args(["hash", "to-sri", "--type", "sha256", hash])
        .output()
        .context(format!(
            "Failed to nix hash to-sri --type sha256 '{hash}'",
            hash = hash
        ))?
        .stdout;
    let res = std::str::from_utf8(&res)
        .context("nix hash output was not utf8")?
        .trim()
        .to_owned();
    if res.is_empty() {
        Err(anyhow!(
            "nix hash to-sri returned empty result. Hash was {}",
            hash
        ))
    } else {
        Ok(res)
    }
}

/// auto discover newest flake rev if you leave it off.
fn lookup_missing_flake_revs(parsed_config: &mut config::ConfigToml) -> Result<()> {
    if let Some(flakes) = &mut parsed_config.flakes {
        let toml = std::fs::read_to_string(parsed_config.anysnake2_toml_path.as_ref().unwrap())
            .expect("Could not reread config file");
        let mut doc = toml.parse::<Document>().expect("invalid doc");

        let mut write = false;
        for (flake_name, flake) in flakes.iter_mut() {
            if flake.rev.is_none() {
                if flake.url.starts_with("github:") {
                    use flake_writer::{add_auth, get_proxy_req};
                    let re = Regex::new("github:/?([^/]+)/([^/?]+)/?([^/?]+)?").unwrap();
                    let out = re
                        .captures_iter(&flake.url)
                        .next()
                        .with_context(|| format!("Could not parse github url {:?}", flake.url))?;
                    let owner = &out[1];
                    let repo = &out[2];
                    let branch = out.get(3).map_or("", |m| m.as_str());
                    let branch = if !branch.is_empty() {
                        Cow::from(branch)
                    } else {
                        let url = format!("https://api.github.com/repos/{}/{}", &owner, repo);
                        let body: String =
                            add_auth(get_proxy_req()?.get(&url)).call()?.into_string()?;
                        let json: serde_json::Value = serde_json::from_str(&body)
                            .context("Failed to parse github repo api")?;
                        let default_branch = json
                            .get("default_branch")
                            .with_context(|| {
                                format!("no default branch in github repos api?! {}", url)
                            })?
                            .as_str()
                            .with_context(|| format!("default branch not a string? {}", url))?;
                        Cow::from(default_branch.to_string())
                    };

                    let branch_url = format!(
                        "https://api.github.com/repos/{}/{}/branches/{}",
                        &owner, &repo, &branch
                    );
                    let body: String = add_auth(get_proxy_req()?.get(&branch_url))
                        .call()?
                        .into_string()?;
                    let json: serde_json::Value =
                        serde_json::from_str(&body).with_context(|| {
                            format!("Failed to parse github repo/branches api {}", branch_url)
                        })?;
                    let commit = json
                        .get("commit")
                        .with_context(|| {
                            format!("no commit in github repo/branches? {}", branch_url)
                        })?
                        .get("sha")
                        .with_context(|| {
                            format!("No sha on github repo/branches/commit? {}", branch_url)
                        })?
                        .as_str()
                        .context("sha not a string?")?;
                    doc["flakes"][flake_name]["rev"] = value(commit);
                    write = true;
                    println!("auto detected head revision for {}", &flake_name);
                    flake.rev = Some(commit.to_string());
                } else if flake.url.starts_with("hg+https:") {
                    let url = if flake.url.contains('?') {
                        flake.url.split_once('?').unwrap().0
                    } else {
                        &flake.url[..]
                    }
                    .strip_prefix("hg+")
                    .unwrap();
                    let rev = discover_newest_rev_hg(url)?;
                    doc["flakes"][flake_name]["rev"] = value(&rev);
                    write = true;
                    println!("auto detected head revision for {}", &flake_name);
                    flake.rev = Some(rev);
                } else if flake.url.starts_with("git+https:") {
                    let url = if flake.url.contains('?') {
                        flake.url.split_once('?').unwrap().0
                    } else {
                        &flake.url[..]
                    }
                    .strip_prefix("git+")
                    .unwrap();
                    let rev = discover_newest_rev_git(url, None)?;
                    doc["flakes"][flake_name]["rev"] = value(&rev);
                    write = true;
                    println!("auto detected head revision for {}", &flake_name);
                    flake.rev = Some(rev);
                } else {
                    bail!(format!("Flake {} must have a rev (auto lookup of newest rev only supported for 'github:' or 'hg+https://' hosted flakes", flake_name));
                }
            }
        }
        if write {
            let out_toml = doc.to_string();
            std::fs::write(
                parsed_config.anysnake2_toml_path.as_ref().unwrap(),
                out_toml,
            )
            .expect("failed to rewrite config file");
            println!("rewriten anysnake2.toml");
        }
    };
    Ok(())
}

fn discover_newest_rev_git(url: &str, branch: Option<&str>) -> Result<String> {
    let rewritten_url = if url.starts_with("github:") {
        url.replace("github:", "https://github.com/")
    } else {
        url.to_string()
    };
    let refs = match branch {
        Some(x) => Cow::from(format!("refs/heads/{}", x)),
        None => Cow::from("HEAD"),
    };
    let output = run_without_ctrl_c(|| {
        //todo: run this is in the provided nixpkgs!
        Ok(std::process::Command::new("git")
            .args(["ls-remote", &rewritten_url, &refs])
            .output()?)
    })
    .expect("git ls-remote failed");
    let stdout =
        std::str::from_utf8(&output.stdout).expect("utf-8 decoding failed  no hg id --debug");
    let hash_re = Regex::new(&format!("^([0-9a-z]{{40}})\\s+{}", &refs)).unwrap(); //hash is on a line together with the ref...
    if let Some(group) = hash_re.captures_iter(stdout).next() {
        return Ok(group[1].to_string());
    }
    Err(anyhow!(
        "Could not find revision hash in 'git ls-remote {} {}' output.{}",
        url,
        refs,
        if branch.is_some() {
            " Is your branchName correct?"
        } else {
            ""
        }
    ))
}

fn discover_newest_rev_hg(url: &str) -> Result<String> {
    let output = run_without_ctrl_c(|| {
        //todo: run this is in the provided nixpkgs!
        Ok(std::process::Command::new("hg")
            .args(["id", "--debug", url, "--id"])
            .output()?)
    })
    .with_context(|| format!("hg id --debug {} failed", url))?;
    let stdout =
        std::str::from_utf8(&output.stdout).expect("utf-8 decoding failed  no hg id --debug");
    let hash_re = Regex::new("(?m)^([0-9a-z]{40})$").unwrap(); //hash is on it's own line.
    if let Some(group) = hash_re.captures_iter(stdout).next() {
        return Ok(group[0].to_string());
    }
    Err(anyhow!(
        "Could not find revision hash in 'hg id --debug {}' output",
        url
    ))
}


fn add_line_numbers(s: &str) -> String {
    let mut out = String::new();
    for (i, line) in s.lines().enumerate() {
        out.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }
    out
}
