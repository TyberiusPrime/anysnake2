#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]

extern crate clap;
use anyhow::{anyhow, bail, Context, Result};
use anysnake2::util::{add_line_numbers, dir_empty, CloneStringLossy};
use anysnake2::{
    install_ctrl_c_handler, run_without_ctrl_c, safe_python_package_name, ErrorWithExitCode,
};
use clap::parser::ValueSource;
use clap::{Arg, ArgMatches};
use config::SafePythonName;
use ex::fs;
use indoc::indoc;
use log::{debug, error, info, trace, warn};
use python_parsing::parse_egg;
use serde::Deserialize;
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{collections::HashMap, str::FromStr};
use tofu::apply_trust_on_first_use;

mod config;
mod flake_writer;
mod python_parsing;
mod tofu;
mod vcs;

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

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    // we wrap  the actual main to enable exit codes.
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
        Ok(()) => {
            std::process::exit(0);
        }
    }
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
                clap::Command::new("oci").about("build OCI container image (anysnake2_container.sif)"),
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
                println!(
                    "{}",
                    std::include_str!("../examples/full/anysnake2.toml")
                        .replace("url = \"dev\"\n", "")
                );
            }
            Some(("basic", _)) => {
                // includes basic
                println!(
                    "{}",
                    std::include_str!("../examples/basic/anysnake2.toml")
                        .replace("url = \"dev\"\n", "")
                );
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
    let verbosity: usize = match str_verbosity {
        Some(str_verbosity) => usize::from_str(str_verbosity)
            .context("Failed to parse verbosity. Must be an integer")?,
        None => default_verbosity,
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

/// We switch to a different version of anysnake2 if the version in the config file is different from the one we are currently running.
/// (unless that's 'dev', or --no-version-switch was passed)
fn switch_to_configured_version(
    parsed_config: &config::TofuMinimalConfigToml,
    matches: &ArgMatches,
) -> Result<()> {
    match &parsed_config.anysnake2.url2 {
        config::TofuVCSorDev::Dev => {
            info!("Using development version of anysnake");
        }
        config::TofuVCSorDev::Vcs(url) => {
            if matches.value_source("no-version-switch") == Some(ValueSource::CommandLine) {
                info!("--no-version-switch was passed, not switching versions");
            } else {
                let rev = match url {
                    vcs::TofuVCS::Git {
                        url: _,
                        branch: _,
                        rev,
                    }
                    | vcs::TofuVCS::GitHub {
                        owner: _,
                        repo: _,
                        branch: _,
                        rev,
                    } => rev,
                    vcs::TofuVCS::Mercurial { .. } => {
                        bail!("Anysnake itself must be hosted on a git repo")
                    }
                };
                if rev.as_str()
                    != matches
                        .get_one::<String>("_running_version")
                        .cloned()
                        .unwrap_or_else(|| "noversionspecified".to_string())
                {
                    info!("restarting with version from {}", url.to_nix_string());
                    let repo = url.to_nix_string();

                    let mut args =
                        vec!["shell", &repo, "-c", "anysnake2", "--_running_version", rev];
                    let input_args: Vec<String> = std::env::args().collect();
                    {
                        for argument in input_args.iter().skip(1) {
                            args.push(argument);
                        }
                        trace!("new args {:?}", args);
                        debug!("running nix {}", &args.join(" "));
                        let status =
                            run_without_ctrl_c(|| Ok(Command::new("nix").args(&args).status()?))?;
                        //now push
                        std::process::exit(status.code().unwrap());
                    }
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::vec_init_then_push)]
#[allow(clippy::too_many_lines)]
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
    let minimal_parsed_config: config::TofuMinimalConfigToml =
        tofu::tofu_anysnake2_itself(minimal_parsed_config)?;

    switch_to_configured_version(&minimal_parsed_config, &matches)?;

    let parsed_config: config::ConfigToml = config::ConfigToml::from_file(&config_file)?;
    if cmd == "version" {
        //output the version you'd actually be using!
        print_version_and_exit();
    }

    let in_non_spec_but_cached_values = load_cached_values(&flake_dir)?;
    let mut out_non_spec_but_cached_values: HashMap<String, String> = HashMap::new();

    let tofued_config = apply_trust_on_first_use(parsed_config)?;

    if cmd == "attach" {
        return attach_to_previous_container(&flake_dir);
    }

    let use_generated_file_instead = tofued_config.anysnake2.do_not_modify_flake;

    if !(tofued_config.cmd.contains_key(cmd) || cmd == "build" || cmd == "run" || cmd == "develop")
    {
        bail!(
            "Cmd {} not found.
            Available from config file: {:?}
            Available from anysnake2: build, run, example-config, version
            ",
            cmd,
            tofued_config.cmd.keys()
        );
    }

    let mut tofued_config = tofued_config;

    let flake_changed = flake_writer::write_flake(
        &flake_dir,
        &mut tofued_config,
        use_generated_file_instead,
        &in_non_spec_but_cached_values,
        &mut out_non_spec_but_cached_values,
    )?;

    if flake_changed.flake_nix_changed && !use_generated_file_instead {
        register_flake_inputs_as_gc_root(&flake_dir)?;
    }

    if out_non_spec_but_cached_values != in_non_spec_but_cached_values {
        save_cached_values(&flake_dir, &out_non_spec_but_cached_values)?;
    }
    let in_non_spec_but_cached_values = out_non_spec_but_cached_values.clone(); // so we don't write
                                                                                // out again if we
                                                                                // don't have to

    perform_clones(&flake_dir, &tofued_config)?;

    if let Some(("build", sc)) = matches.subcommand() {
        {
            match sc.subcommand() {
                Some(("flake", _)) => {
                    info!("Writing just flake/flake.nix");
                    rebuild_flake(
                        use_generated_file_instead,
                        "flake",
                        &flake_dir,
                        flake_changed.flake_nix_changed,
                    )?;
                }
                Some(("oci", _)) => {
                    info!("Building oci-image in flake/result");
                    rebuild_flake(
                        use_generated_file_instead,
                        "oci_image",
                        &flake_dir,
                        flake_changed.flake_nix_changed,
                    )?;
                }
                Some(("rootfs", _)) => {
                    info!("Building rootfs in flake/result");
                    rebuild_flake(
                        use_generated_file_instead,
                        "",
                        &flake_dir,
                        flake_changed.flake_nix_changed,
                    )?;
                }
                _ => {
                    info!("Please pass a subcommand as to what to build (use --help to list)");
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
                &tofued_config.dev_shell.shell
            ),
        )
        .context("Failed to write run.sh")?; // the -i makes it read /etc/bashrc

        let build_output: PathBuf = flake_dir.join("result/rootfs");
        let build_unfinished_file = flake_dir.join(".build_unfinished"); // ie. the flake build failed
                                                                         //
                                                                         //early error exit if you try to run an non-existant command
        if flake_changed.flake_nix_changed
            || flake_changed.python_lock_changed
            || !build_output.exists()
            || build_unfinished_file.exists()
        {
            info!("Rebuilding flake");
            rebuild_flake(
                use_generated_file_instead,
                "",
                &flake_dir,
                flake_changed.flake_nix_changed,
            )?;
        }

        if let Some(python) = &tofued_config.python {
            //todo
            fill_venv(&python.version, &python.packages, &flake_dir)?;
            /* if let Some(r) = &tofued_config.r {
                add_r_library_path(
                    &flake_dir,
                    r,
                    &mut tofued_config.container,
                    &in_non_spec_but_cached_values,
                    &mut out_non_spec_but_cached_values,
                )?;
            } */
        };
        if out_non_spec_but_cached_values != in_non_spec_but_cached_values {
            save_cached_values(&flake_dir, &out_non_spec_but_cached_values)?;
        }
        if cmd == "develop" {
            if let Some(python) = &tofued_config.python {
                write_develop_python_path(&flake_dir, &python.packages, &python.version)?;
            }
            run_without_ctrl_c(|| {
                let s = format!("../{}", &run_sh_str);
                let full_args = vec!["develop", "-c", "bash", &s];
                info!("{:?}", full_args);
                Ok(Command::new("nix")
                    .current_dir(&flake_dir)
                    .args(full_args)
                    .status()?)
            })?;
        } else {
            let home_dir = PathBuf::from(replace_env_vars(
                tofued_config.container.home.as_deref().unwrap_or("$HOME"),
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
                let cmd_info = tofued_config.cmd.get(cmd).context("Command not found")?;
                if let Some(bash_script) = &cmd_info.pre_run_outside {
                    info!("Running pre_run_outside for cmd - cmd {}", cmd);
                    run_bash(bash_script).with_context(|| {
                        format!(
                            "pre run outside failed. Script:\n{}",
                            add_line_numbers(bash_script)
                        )
                    })?;
                };
                if let Some(while_run_outside) = &cmd_info.while_run_outside {
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
                post_run_outside.clone_from(&cmd_info.post_run_outside);
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
            if let Some(python) = &tofued_config.python {
                let venv_dir: PathBuf = flake_dir.join("venv").join(&python.version);
                error!("{:?}", venv_dir);
                let mut python_paths = Vec::new();
                for (pkg, spec) in python
                    .packages
                    .iter()
                    .filter(|(_, spec)| spec.editable_path.is_some())
                {
                    let target_dir: PathBuf = [spec.editable_path.as_ref().unwrap(), pkg.as_str()]
                        .iter()
                        .collect(); //todo: make configurable
                    binds.push((
                        target_dir.to_string_lossy(),
                        format!("/anysnake2/venv/linked_in/{pkg}"),
                        "ro".to_string(),
                    ));

                    let egg_link = venv_dir.join(format!("{pkg}.venv-link"));
                    python_paths.push(parse_egg(egg_link)?);
                }
                if !python_paths.is_empty() {
                    envs.push(format!("PYTHONPATH={}", python_paths.join(":")));
                }
                paths.push("/anysnake2/venv/bin");
            };

            if let Some(volumes_ro) = &tofued_config.container.volumes_ro {
                for (from, to) in volumes_ro {
                    let from: PathBuf = fs::canonicalize(from).context(format!(
                        "canonicalize path failed on {} (read only volume - does the path exist?)",
                        &from
                    ))?;
                    let from = from.into_os_string().to_string_lossy().to_string();
                    binds.push((from, to.to_string(), "ro".to_string()));
                }
            };
            if let Some(volumes_rw) = &tofued_config.container.volumes_rw {
                for (from, to) in volumes_rw {
                    let from: PathBuf = fs::canonicalize(from).context(format!(
                        "canonicalize path failed on {} (read/write volume - does the path exist?)",
                        &from
                    ))?;
                    let from = from.into_os_string().to_string_lossy().to_string();
                    binds.push((from, to.to_string(), "rw".to_string()));
                }
            }
            for (from, to, opts) in binds {
                singularity_args.push("--bind".into());
                singularity_args.push(format!("{from}:{to}:{opts}",));
            }

            if let Some(container_envs) = &tofued_config.container.env {
                for (k, v) in container_envs {
                    envs.push(format!("{}={}", k, replace_env_vars(v)));
                }
            }

            envs.push(format!("PATH={}", paths.join(":")));

            for e in envs {
                singularity_args.push("--env".into());
                singularity_args.push(e);
            }

            singularity_args.push(flake_dir.join("result/rootfs").to_string_lossy());
            singularity_args.push("/bin/bash".into());
            singularity_args.push("/anysnake2/outer_run.sh".into());
            for s in top_level_slop.iter().skip(1) {
                singularity_args.push(s.to_string());
            }
            let dtach_socket = match &tofued_config.anysnake2.dtach {
                true => {
                    if std::env::var("STY").is_err() && std::env::var("TMUX").is_err() {
                        Some(format!(
                            "{}_{}",
                            cmd,
                            jiff::Zoned::now().datetime()
                        ))
                    } else {
                        None
                    }
                }
                false => None,
            };

            let singularity_result = run_singularity(
                &singularity_args[..],
                Some(&run_dir.join("singularity.bash")),
                dtach_socket.as_ref(),
                &flake_dir,
            )?;
            if let Some(bash_script) = post_run_outside {
                if let Err(e) = run_bash(&bash_script) {
                    warn!(
                        "An error occured when running the post_run_outside bash script: {}\nScript: {}",
                        e,
                        add_line_numbers(&bash_script)
                    );
                }
            };
            if let Some(mut parallel_running_child) = parallel_running_child {
                parallel_running_child
                    .kill()
                    .context("Failed to kill parallel running child")?;
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

/// run a process inside a singularity container.
fn run_singularity(
    args: &[String],
    log_file: Option<&PathBuf>,
    dtach_socket: Option<&String>,
    flake_dir: &Path,
) -> Result<std::process::ExitStatus> {
    let singularity_url = format!(
        "{}#singularity",
        anysnake2::get_outside_nixpkgs_url().unwrap()
    );
    register_nix_gc_root(&singularity_url, flake_dir)?;
    run_without_ctrl_c(|| {
        let mut nix_full_args: Vec<String> = Vec::new();
        let using_dtach = if let Some(dtach_socket) = &dtach_socket {
            let dtach_dir = flake_dir.join("dtach");
            fs::create_dir_all(dtach_dir)?;
            let dtach_url = format!("{}#dtach", anysnake2::get_outside_nixpkgs_url().unwrap());

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
            "shell".into(),
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
    info!("anysnake2 version: {}", VERSION);
    std::process::exit(0);
}

fn pretty_print_singularity_call(args: &[String]) -> String {
    let mut res = String::new();
    let mut skip_space = false;
    for arg in args {
        if skip_space {
            skip_space = false;
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
        if arg == "--bind" || arg == "--env" || arg == "--home" || arg == "singularity" {
            skip_space = true;
            res += " ";
        } else {
            res += " \\\n";
        }
    }
    res.pop();
    res += "\n";
    res
}

fn extract_python_package_version_from_uv_lock(
    flake_dir: &Path,
    safe_name: &str,
) -> Result<String> {
    let uv_lock_path: PathBuf = flake_dir.join("uv/uv.lock");
    let uv_lock_raw = fs::read_to_string(&uv_lock_path)?;
    let uv_lock: toml::Value = toml::from_str(&uv_lock_raw)?;
    for package in uv_lock["package"]
        .as_array()
        .expect("uv.lock parsing error")
    {
        if package["name"]
            .as_str()
            .expect("no package name/not a string in uv.lock")
            == safe_name
        {
            return Ok(package["version"]
                .as_str()
                .expect("package version not a string in uv.lock")
                .to_string());
        }
    }
    bail!("Could not find package {} in uv.lock", safe_name);
}

fn download_and_unzip(url: &str, target_dir: &Path) -> Result<()> {
    //remove target dir if it exists
    if target_dir.exists() {
        ex::fs::remove_dir_all(target_dir).context("Failed to remove target dir")?;
    }
    ex::fs::create_dir_all(target_dir).context("Failed to create target dir")?;
    let download_filename = target_dir.join("download.tar.gz");

    {
        let tf = ex::fs::File::create(&download_filename)?;
        let mut btf = std::io::BufWriter::new(tf);
        let mut req = anysnake2::util::get_proxy_req()?.get(url).call()?;
        std::io::copy(&mut req.body_mut().as_reader(), &mut btf)?;
    }
    //call tar to unpack
    Command::new("tar")
        .args(["-xzf", "download.tar.gz", "--strip-components=1"])
        .current_dir(target_dir)
        .status()
        .context("Failed to untar downloaded archive")?;

    ex::fs::remove_file(download_filename).context("Failed to remove download file")?;

    Ok(())
}

fn clone(
    flake_dir: &Path,
    parent_dir: &str,
    name: &str,
    source: &config::TofuPythonPackageSource,
    known_clones: &mut HashMap<String, String>,
) -> Result<()> {
    let final_dir: PathBuf = [parent_dir, name].iter().collect();
    fs::create_dir_all(&final_dir)?;
    if dir_empty(&final_dir)? {
        info!("cloning {}/{} from {:?}", parent_dir, name, source);
        match source {
            config::TofuPythonPackageSource::PyPi { .. }
            | config::TofuPythonPackageSource::VersionConstraint(_) => {
                let safe_name = safe_python_package_name(name);
                let actual_version =
                    extract_python_package_version_from_uv_lock(flake_dir, &safe_name)?;
                // I don't see how we get from what's in poetry.lock to the url right now, and this
                // is at hand
                let url =
                    anysnake2::util::get_pypi_package_source_url(&safe_name, Some(&actual_version))
                        .context("Failed to get python package source")?;
                download_and_unzip(&url, &final_dir)?;
            }
            config::TofuPythonPackageSource::Url(url) => {
                download_and_unzip(url, &final_dir)?;
            }
            config::TofuPythonPackageSource::Vcs(tofu_vcs) => {
                tofu_vcs.clone_repo(&final_dir.to_string_lossy())?;
            }
        }
        known_clones.insert(name.to_string(), source.to_string());
    }
    Ok(())
}

fn jujutsu_init(git_repo_dir: &Path) -> Result<()> {
    let dtach_url = format!("{}#jujutsu", anysnake2::get_outside_nixpkgs_url().unwrap());
    let nix_full_args = vec!["shell", &dtach_url, "-c", "jj", "git", "init", "--colocate"];
    let status = Command::new("nix")
        .args(nix_full_args)
        .current_dir(git_repo_dir)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("jujustu init failed"))
    }
}

fn perform_clones(flake_dir: &Path, parsed_config: &config::TofuConfigToml) -> Result<()> {
    let do_jujustu = parsed_config.clone_options.jujutsu;
    // the old school 'clones' clones
    let mut todo: HashMap<String, HashMap<String, config::TofuPythonPackageSource>> =
        HashMap::new();
    if let Some(clones) = parsed_config.clones.as_ref() {
        for (target_dir, entries) in clones {
            for (name, source) in entries {
                let entry = todo
                    .entry(target_dir.to_string())
                    .or_default();
                entry.insert(
                    name.clone(),
                    config::TofuPythonPackageSource::Vcs(source.clone()),
                );
            }
        }
    }
    //now add in editable python packages
    if let Some(python) = &parsed_config.python {
        for (pkg_name, package) in &python.packages {
            if let Some(editable_path) = &package.editable_path {
                let entry = todo
                    .entry(editable_path.to_string())
                    .or_default();
                let safe_name = pkg_name.to_string();
                entry.insert(safe_name, package.source.clone());
            }
        }
    }

    for (target_dir, name_urls) in &todo {
        fs::create_dir_all(target_dir).context(format!("Could not create {target_dir}"))?;
        let clone_log: PathBuf = [target_dir, ".clone_info.json"].iter().collect();
        let mut known_clones: HashMap<String, String> = if clone_log.exists() {
            serde_json::from_str(&fs::read_to_string(&clone_log)?)?
        } else {
            HashMap::new()
        };
        let do_clones = |known_clones: &mut HashMap<String, String>| {
            for (name, source) in name_urls {
                let known_source = known_clones.get(name).map_or("", String::as_str);
                let final_dir: PathBuf = [target_dir, name].iter().collect();
                let new_source_str = format!("{}", source.without_username_in_url());
                if final_dir.exists() && !dir_empty(&final_dir)? && known_source != new_source_str
                //empty dir is ok.
                {
                    let msg = format!(
                            "Url changed for clone target: {target_dir}/{name}. Was '{known_source}' is now '{new_source_str}'.\n\
                        Cowardly refusing to throw away old checkout in {final_dir:?}."
                        );
                    bail!(msg);
                }
            }
            for (name, url) in name_urls {
                clone(flake_dir, target_dir, name, url, known_clones).with_context(|| {
                    format!("Cloning for {name} into {target_dir} from {url:?}")
                })?;
                if do_jujustu {
                    jujutsu_init(&([target_dir, name].iter().collect::<PathBuf>()))?;
                }
            }
            Ok(())
        };
        let clone_result = do_clones(&mut known_clones);
        fs::write(
            &clone_log,
            serde_json::to_string_pretty(&json!(known_clones))?,
        )
        .with_context(|| format!("Failed to write {:?}", &clone_log))?;
        clone_result?;
    }

    Ok(())
}

fn rebuild_flake(
    use_generated_file_instead: bool,
    target: &str,
    flake_dir: impl AsRef<Path>,
    flake_content_changed: bool,
) -> Result<()> {
    debug!("writing flake");

    if !use_generated_file_instead {
        if flake_content_changed {
            debug!("flake content changed, relocking to avoid locking-path-dependencies");
            let flake_lock_path = flake_dir.as_ref().join("flake.lock");
            if flake_lock_path.exists() {
                fs::remove_file(&flake_lock_path)?;
            }
            Command::new("nix")
                .args(["flake", "lock"])
                .current_dir(&flake_dir)
                .status()
                .context("(Re)-locking flake failed")?;
        }
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

    if target == "flake" {
        Ok(())
    } else {
        debug!("building container");
        let nix_build_result =
            Command::new("nix")
                .args(["build", &format!("./#{target}"), "-v",
                "--max-jobs", "auto",
                "--cores", "4",
                "--keep-going"
                ]
                )
                .current_dir(&flake_dir)
                .status()
                .with_context(|| format!("nix build failed. Perhaps try with --show-trace using 'nix build ./#{target} -v --show-trace'"))?;
        if nix_build_result.success() {
            fs::remove_file(&build_unfinished_file)?;
            Ok(())
        } else {
            Err(anyhow!("flake building failed"))
        }
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

/// so we can use `${env_var}` in the home dir, and export envs into the containers
fn replace_env_vars(input: &str) -> String {
    let mut output = input.to_string();
    for (k, v) in std::env::vars() {
        output = output.replace(&format!("${k}"), &v);
        output = output.replace(&format!("${{{k}}}"), &v);
    }
    output
}

// deal with the editable packages.
fn fill_venv(
    python_version: &str,
    python: &HashMap<SafePythonName, config::TofuPythonPackageDefinition>,
    flake_dir: &Path,
) -> Result<()> {
    let venv_dir: PathBuf = flake_dir.join("venv").join(python_version);
    fs::create_dir_all(venv_dir.join("bin"))?;
    fs::create_dir_all(flake_dir.join("venv_develop"))?;
    let mut to_build = Vec::new();
    let mut to_rewrite_python_shebang = Vec::new();

    let target_python: PathBuf = PathBuf::from_str(".anysnake2_flake/result/rootfs/bin/python")
        .unwrap()
        .canonicalize()
        .context("failed to find python binary in container")?;
    let target_python_str = target_python.to_string_lossy();

    for (pkg, spec) in python
        .iter()
        .filter(|(_pkg, spec)| spec.editable_path.is_some())
    {
        debug!("ensuring venv  for {pkg}");
        let target_dir: PathBuf = [spec.editable_path.as_ref().unwrap(), pkg.as_str()]
            .iter()
            .collect();
        if !target_dir.exists() {
            bail!("editable python package that was not present in file system (missing clone)? looking for package {} in {:?}",
                               pkg, target_dir);
        }
        let venv_link = venv_dir.join(format!("{pkg}.venv-link"));

        let venv_used = {
            let anysnake_link = venv_dir.join(format!("{pkg}.anysnake-link"));
            if anysnake_link.exists() {
                ex::fs::read_to_string(anysnake_link)?
            } else {
                String::new()
            }
        };
        if !venv_link.exists() {
            // so that changing python versions triggers a rebuild.
            to_build.push((pkg, target_dir));
        } else if venv_used != target_python_str {
            to_rewrite_python_shebang.push((pkg, target_dir));
        }
    }
    for (safe_pkg, target_dir) in &to_build {
        install_editable_into_venv(
            safe_pkg,
            target_dir,
            &target_python,
            &target_python_str,
            &venv_dir,
            flake_dir,
            python_version,
        )?;
    }
    if !to_rewrite_python_shebang.is_empty() {
        let mut old_pythons = HashSet::new();
        for (safe_pkg, _target_dir) in &to_rewrite_python_shebang {
            let anysnake_link = venv_dir.join(format!("{safe_pkg}.anysnake-link"));
            if anysnake_link.exists() {
                old_pythons.insert(format!(
                    "#!{}",
                    ex::fs::read_to_string(&anysnake_link)?.trim()
                ));
            }
        }

        for bin_file in fs::read_dir(venv_dir.join("bin"))? {
            let bin_file = bin_file?;
            let old_content = fs::read_to_string(bin_file.path())?;
            let first_line = old_content.lines().next().unwrap_or("");
            if first_line.starts_with("#!") && old_pythons.contains(first_line) {
                let new_content =
                    old_content.replace(first_line, &format!("#!{target_python_str}"));
                fs::write(bin_file.path(), new_content).context("failed to write to file")?;
            }
        }
        // we only update the anysnake link after fixing all the bin files
        // so we'd attempt it again if it was aborted
        for (safe_pkg, _target_dir) in &to_rewrite_python_shebang {
            let anysnake_link = venv_dir.join(format!("{safe_pkg}.anysnake-link"));

            fs::write(anysnake_link, &target_python_str)
                .context("target anysnake link write failed")?;
        }
    }
    Ok(())
}

fn install_editable_into_venv(
    safe_pkg: &SafePythonName,
    target_dir: &PathBuf,
    target_python: &PathBuf,
    target_python_str: &str,
    venv_dir: &Path,
    flake_dir: &Path,
    python_version: &str,
) -> Result<()> {
    {
        info!("Pip install {:?}", &target_dir);
        let td = tempfile::Builder::new().prefix("anysnake_venv").tempdir()?; // temp /tmp
        let td_home = tempfile::Builder::new().prefix("anysnake_venv").tempdir()?; // temp home directory
        let td_home_str = td_home.path().to_string_lossy().to_string();

        let search_python = target_python.to_string_lossy();
        debug!("target_python {:?}", target_python);
        debug!("search_python {:?}", search_python);
        let pkg_python_name = safe_pkg.to_python_name();

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
                python <<EOT
                from pathlib import Path
                for fn in Path('/tmp/venv/bin').glob('*'):
                    input = fn.read_text()
                    if '{search_python}' in input:
                        output = input.replace('{search_python}', '{target_python}')
                        fn.write_text(output)
                try:
                    import {pkg_python_name}
                    import sys
                    assert len(sys.modules['{pkg_python_name}'].__path__) == 1, 'module.__path__ had more than one entry. Not sure when this happens, file a bug report with the module that is giving you trouble'
                    module_path = Path(sys.modules['{pkg_python_name}'].__path__[0]).parent
                    Path('/anysnake2/venv/{safe_pkg}.venv-link').write_text(str(module_path))
                except ImportError:
                    print('package name did not match module name for {safe_pkg}/{pkg_python_name}')
                EOT
                cp /tmp/venv/bin/* /anysnake2/venv/bin 2>/dev/null|| true
               "
            ),
            safe_pkg = &safe_pkg,
            search_python = search_python,
            target_python = &target_python_str,
            pkg_python_name = pkg_python_name
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
                venv_dir.to_string_lossy()
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

        let target_venv_link = venv_dir.join(format!("{safe_pkg}.venv-link"));
        if !target_venv_link.exists() {
            //ok, we failed because the python module name and package name didn't match.
            //info!("target_venv_link did not exist");
            let source_egg_folder = td
                .path()
                .join("venv/lib")
                .join(format!("python{python_version}"))
                .join("site-packages");
            let paths = fs::read_dir(&source_egg_folder)?;
            for path in paths {
                let path = path.unwrap().path();
                //info!("path: {:?}", path);
                let suffix = path.extension().map_or_else(
                    || Cow::Owned(String::default()),
                    std::ffi::OsStr::to_string_lossy,
                );
                if suffix == "egg-link" {
                    let content = parse_egg(&path)?;
                    fs::write(&target_venv_link, format!("{content}\n"))?;
                    break;
                } else if suffix == "pth" {
                    //we want to read {safe_pkg}.egg-link, not __editable__{safe_pkg}-{version}.pth
                    //because we don't *know* the version
                    //and this happens only once
                    let content = parse_egg(&path)?;
                    if content.starts_with("/anysnake") {
                        //the easy case, it's just a path.
                        fs::write(&target_venv_link, format!("{content}\n"))?;
                        break;
                    }
                }
            }
        }
        // still not there?
        if !target_venv_link.exists() {
            bail!("venv build did not write the expected venv-link, and it was not fixed by fallbacks. File a bugreport");
        }

        let target_anysnake_link = venv_dir.join(format!("{safe_pkg}.anysnake-link"));
        fs::write(target_anysnake_link, target_python_str)
            .context("target anysnake link write failed")?;
    }
    Ok(())
}

/* /// the R 'binary' itself set's it's `LD_LIBRARY_PATH`,
/// but for e.g. rpy2 to work correctly, we need to set the correct `LD_LIBRARY_PATH`
/// inside the container
/// fortunatly, we can ask R about it
///
/// 20240913: I am no longer convinced this is necessary and useful
/// rpy2 seems to be working without and it breaks STAR in the full flake
fn add_r_library_path(
    flake_dir: &Path,
    r: &config::TofuR,
    container: &mut config::Container,
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
) -> Result<()> {
    use std::collections::hash_map::Entry;
    let key = format!("r_ld_path~{}~{}", sha256::digest(r.url.to_string()), r.date);
    #[allow(clippy::single_match_else)]
    let ld_library_path = match in_non_spec_but_cached_values.get(&key) {
        Some(ld_library_path) => ld_library_path.clone(),
        None => {
            let ld_library_path = figure_out_r_library_path(flake_dir)?;
            out_non_spec_but_cached_values.insert(key, ld_library_path.clone());
            ld_library_path
        }
    };
    match &mut container.env {
        Some(env) => match env.entry("LD_LIBRARY_PATH".to_string()) {
            Entry::Occupied(mut e) => {
                let current = e.get_mut();
                current.push(':');
                current.push_str(&ld_library_path);
            }
            Entry::Vacant(e) => {
                e.insert(ld_library_path);
            }
        },
        None => {
            let mut env = HashMap::new();
            env.insert("LD_LIBRARY_PATH".to_string(), ld_library_path);
            container.env = Some(env);
        }
    }
    Ok(())
} */

/* fn figure_out_r_library_path(flake_dir: &Path) -> Result<String> {
    let singularity_url = format!("{}#singularity", anysnake2::get_outside_nixpkgs_url().unwrap());
    let singularity_args: Vec<String> = vec![
        "shell".into(),
        singularity_url,
        "-c".into(),
        "singularity".into(),
        "exec".into(),
        "--userns".into(),
        "--cleanenv".into(),
        //"--no-home".into(),
        "--no-home".into(),
        "--bind".into(),
        "/nix/store:/nix/store:ro".into(),
        flake_dir.join("result/rootfs").to_string_lossy(),
        "Rscript".into(),
        "-e".into(),
        "cat(Sys.getenv(\"LD_LIBRARY_PATH\"))".into(),
    ];
    let cmd = Command::new("nix")
        .args(&singularity_args[..])
        .output()
        .context("Failed to run nix")?;
    info!("querying Singularity for R ld_library_path ");
    let stdout = std::str::from_utf8(&cmd.stdout).unwrap();
    let stderr = std::str::from_utf8(&cmd.stderr).unwrap();
    if !cmd.status.success() {
        bail!(
            "Singularity querying R ld_library_path failed: return code {:?}. Stderr: {:?}",
            cmd.status.code().unwrap(),
            stderr
        );
    }
    let ld_libarry_path = stdout.trim();
    Ok(ld_libarry_path.to_string())
} */


#[allow(non_snake_case)]
#[derive(Deserialize, Debug)]
struct NixFlakePrefetchOutput {
    storePath: String,
}

#[derive(Deserialize, Debug)]
struct NixBuildOutputs {
    #[serde(alias = "bin", alias = "out")]
    out: String,
}

#[derive(Deserialize, Debug)]
struct NixBuildOutput {
    outputs: NixBuildOutputs,
}

#[test]
fn test_nix_build_output_parsing() {
    let json = r#"[
        {"drvPath":"/nix/store/kg8wnjpwyrr7nkdl64iiakzdmz6hv6d5-nixfmt-0.6.0.drv","outputs":{"bin":"/nix/store/7mr87xfsrc2rn4pkmvrvj9a4lnrwkyks-nixfmt-0.6.0-bin"}},
        {"drvPath":"/nix/store/kg8wnjpwyrr7nkdl64iiakzdmz6hv6d5-nixfmt-0.6.0.drv","outputs":{"out":"/nix/store/7mr87xfsrc2rn4pkmvrvj9a4lnrwkyks-nixfmt-0.6.0-bin"}}
    ]"#;
    let _: Vec<NixBuildOutput> = serde_json::from_str(json).unwrap();
}

fn prefetch_flake(url_without_hash: &str) -> Result<String> {
    debug!("nix prefetching flake {}", &url_without_hash);
    run_without_ctrl_c(|| {
        let output = std::process::Command::new("nix")
            .args(["flake", "prefetch", url_without_hash, "--json"])
            .output()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let j: NixFlakePrefetchOutput = serde_json::from_str(&stdout)?;
            //now from the gc_dir
            Ok(j.storePath)
        } else {
            error!("nix prefetch failed");
            error!("stderr was: {}", String::from_utf8_lossy(&output.stderr));
            Err(anyhow!("nix prefetch failed"))
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
        if output.status.success() {
            Ok(())
        } else {
            Err(anyhow!("nix-store realise failed"))
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
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            info!("{}", stdout);
            let j: Vec<NixBuildOutput> =
                serde_json::from_str(&stdout).context("failed to parse nix build output")?;
            let j = j.into_iter().next().unwrap();
            Ok(j.outputs.out)
        } else {
            Err(anyhow!("nix build failed"))
        }
    })
}

/// register the used tools and the flake itself as gcroots
/// our own flake is automatically gc rooted.
/// and the flake-inputs are handled when/if the flake changed
pub fn register_nix_gc_root(url: &str, flake_dir: impl AsRef<Path>) -> Result<()> {
    debug!("registering gc root for {}", url);
    //where we store this stuff
    let gc_roots = flake_dir.as_ref().join(".gcroots");
    fs::create_dir_all(&gc_roots)?;

    let (without_hash, _) = url
        .rsplit_once('#')
        .context("GC_root url should contain #")?;
    //first we store and hash the flake itself and record that.
    let flake_symlink_here = gc_roots.join(without_hash.replace('/', "_"));
    if !flake_symlink_here.exists() {
        let store_path = prefetch_flake(without_hash)?;
        register_gc_root(&store_path, &flake_symlink_here)?;
    }

    //then we record it's output
    let build_symlink_here = gc_roots.join(url.replace('/', "_"));
    if !build_symlink_here.exists() {
        let store_path = nix_build_flake(url)?;
        register_gc_root(&store_path, &build_symlink_here)?;
    }
    Ok(())
}

fn register_flake_inputs_as_gc_root(flake_dir: impl AsRef<Path>) -> Result<()> {
    //run nix build .#flake_inputs_for_gc_root wit han output dir
    Command::new("nix")
        .args([
            "build",
            ".#flake_inputs_for_gc_root",
            "-o",
            ".gcroot_for_flake_inputs",
        ])
        .current_dir(flake_dir)
        .status()
        .context("retrieving content for flake input gc root from nix failed")?;
    Ok(())
}

fn attach_to_previous_container(flake_dir: impl AsRef<Path>) -> Result<()> {
    let mut available: Vec<_> = fs::read_dir(flake_dir.as_ref().join("dtach"))
        .context("Could not find dtach socket directory")?
        .filter_map(Result::ok)
        .collect();
    if available.is_empty() {
        bail!("No session to attach to available");
    } else if available.len() == 1 {
        info!("reattaching to {:?}", available[0].file_name());
        run_dtach(available[0].path())
    } else {
        available.sort_unstable_by_key(|x| x.file_name());
        loop {
            println!("please choose an entry to reattach (number+enter), or ctrl-c to abort");
            for (ii, entry) in available.iter().enumerate() {
                println!("\t{} {:?}", ii, entry.file_name());
            }
            let line1 = std::io::stdin().lock().lines().next().unwrap().unwrap();
            for (ii, entry) in available.iter().enumerate() {
                if format!("{ii}") == line1 {
                    return run_dtach(entry.path());
                }
            }
            println!("sorry I did not understand that. \n");
        }
    }
}

fn run_dtach(p: impl AsRef<Path>) -> Result<()> {
    let dtach_url = format!("{}#dtach", anysnake2::get_outside_nixpkgs_url().unwrap());
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

#[allow(unused)] //todo, there's a missing code path in 'develop'
fn write_develop_python_path(
    flake_dir: impl AsRef<Path>,
    python_packages: &HashMap<SafePythonName, config::TofuPythonPackageDefinition>,
    python_version: &str,
) -> Result<()> {
    let mut develop_python_paths = Vec::new();
    let venv_dir: PathBuf = flake_dir.as_ref().join("venv").join(python_version);
    let parent_dir: PathBuf = fs::canonicalize(&flake_dir)?
        .parent()
        .context("No parent found for flake dir")?
        .to_path_buf();

    for (pkg, _spec) in python_packages
        .iter()
        .filter(|(_pkg, spec)| spec.editable_path.is_some())
    {
        let real_target = parent_dir.join("code").join(pkg.as_str());
        let egg_link = venv_dir.join(format!("{pkg}.egg-link"));
        let egg_target = parse_egg(egg_link)?;
        let egg_target =
            egg_target.replace("/anysnake2/venv/linked_in", &real_target.to_string_lossy());

        develop_python_paths.push(egg_target);
    }
    fs::write(
        flake_dir.as_ref().join("develop_python_path.bash"),
        format!("export PYTHONPATH=\"{}\"", &develop_python_paths.join(":")),
    )?;
    Ok(())
}

fn load_cached_values(flake_dir: &Path) -> Result<HashMap<String, String>> {
    let filename = flake_dir.join("cached.json");
    Ok(ex::fs::read_to_string(filename)
        .map_or_else(|_| Ok(HashMap::new()), |raw| serde_json::from_str(&raw))?)
}

fn save_cached_values(
    flake_dir: &Path,
    out_non_spec_but_cached_values: &HashMap<String, String>,
) -> Result<()> {
    let out: String = serde_json::to_string_pretty(&out_non_spec_but_cached_values)?;
    ex::fs::write(flake_dir.join("cached.json"), out)?;
    Ok(())
}
