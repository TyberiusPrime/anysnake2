extern crate clap;
use anyhow::{anyhow, bail, Context, Result};
use clap::{value_t, App, AppSettings, Arg, ArgMatches, SubCommand};
use config::PythonPackageDefinition;
use ex::fs;
use lazy_static::lazy_static;
use log::{debug, error, info, trace, warn};
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

fn parse_args() -> ArgMatches<'static> {
    App::new("Anysnake2")
        .version(VERSION)
        .author("Florian Finkernagel <finkernagel@imt.uni-marburg.de>")
        .about("Sane version declaration and container generation using nix")
        .setting(AppSettings::AllowExternalSubcommands)
        .arg(
            //Arg::with_name("no-version-switch")
            Arg::from_usage("--no-version-switch 'do not change to toml file defined version'")
            )
        .arg(
            Arg::with_name("config_file")
                .short("c")
                .long("config")
                .value_name("FILE")
                .help("Sets a custom config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("verbose")
                .short("v")
                .long("verbose")
                .takes_value(true)
                //.default_value("2")
                .help("Sets the level of verbosity (0=quiet,1=error/warnings, 2=info (default), 3=debug, 4=trace, 5=trace)"),
        )
        .arg(
            Arg::with_name("_running_version")
                .long("_running_version")
                .help("internal use only")
                .hidden(true)
                .takes_value(true),
        )
        .subcommand(
            SubCommand::with_name("build").about("build containers (see subcommands), but do not run anything")
            .subcommand(
                SubCommand::with_name("flake").about("write just the flake, but don't nix build anything"),
            )
            .subcommand(
                SubCommand::with_name("rootfs").about("build rootfs container (used for singularity)"),
            )
            .subcommand(
                SubCommand::with_name("sif").about("build SIF (singularity) container image (anysnake2_container.sif)"),
            )

        )
        .subcommand(
            SubCommand::with_name("config")
                .about("dump different example anysnake2.toml to stdout")
                .subcommand(SubCommand::with_name("basic"))
                .subcommand(SubCommand::with_name("minimal"))
                .subcommand(SubCommand::with_name("full"))
        )
        .subcommand(SubCommand::with_name("develop").about("run nix develop, and go back to this dir with your favourite shell"))
        .subcommand(SubCommand::with_name("version").about("the version actually used by the config file. Error if no config file is present (use --version for the version of this binary"))
        .subcommand(SubCommand::with_name("attach").about("attach to previously running session"))
        .subcommand(
            SubCommand::with_name("run")
                .about("run arbitray commands in container (w/o any pre/post bash scripts)")
                .arg(
                    Arg::with_name("slop").takes_value(true).multiple(true), //.last(true), // Indicates that `slop` is only accessible after `--`.
                ),
        )
        .arg(
            Arg::with_name("slop").takes_value(true).multiple(true), //.last(true), // Indicates that `slop` is only accessible after `--`.
        ) //todo: argument passing to the scripts? 
        .get_matches()
}

fn handle_config_command(matches: &ArgMatches<'static>) -> Result<()> {
    if let ("config", Some(sc)) = matches.subcommand() {
        {
            match sc.subcommand().0 {
                "minimal" => println!(
                    "{}",
                    std::include_str!("../examples/minimal/anysnake2.toml")
                ),
                "full" => println!("{}", std::include_str!("../examples/full/anysnake2.toml")),
                "basic" => {
                    // includes basic
                    println!("{}", std::include_str!("../examples/basic/anysnake2.toml"))
                }
                _ => {
                    bail!("Could not find that config. Try to pass minimial/basic/full as in  'anysnake2 config basic'");
                }
            }
            std::process::exit(0);
        }
    }
    Ok(())
}

fn configure_logging(matches: &ArgMatches<'static>) {
    let verbosity = value_t!(matches, "verbose", usize).unwrap_or(2);
    stderrlog::new()
        .module(module_path!())
        .quiet(verbosity == 0)
        .verbosity(verbosity)
        .show_level(false)
        .timestamp(stderrlog::Timestamp::Off)
        .init()
        .unwrap();
}

fn read_config(matches: &ArgMatches<'static>) -> Result<config::ConfigToml> {
    let config_file = matches.value_of("config_file").unwrap_or("anysnake2.toml");
    let abs_config_path = fs::canonicalize(config_file).context("Could not find config file")?;
    let raw_config = fs::read_to_string(&abs_config_path).context("Could not read config file")?;
    let mut parsed_config: config::ConfigToml = config::ConfigToml::from_str(&raw_config)
        .with_context(|| {
            ErrorWithExitCode::new(65, format!("Failure parsing {:?}", &abs_config_path))
        })?;
    parsed_config.anysnake2_toml_path = Some(abs_config_path);
    parsed_config.source = config_file.into();
    Ok(parsed_config)
}

fn switch_to_configured_version(
    parsed_config: &config::ConfigToml,
    matches: &ArgMatches<'static>,
    flake_dir: impl AsRef<Path>,
) -> Result<()> {
    if parsed_config.anysnake2.rev == "dev" {
        info!("Using development version of anysnake");
    } else if matches.is_present("no-version-switch") {
        info!("--no-version-switch was passed, not switching versions");
    } else if parsed_config.anysnake2.rev
        != matches
            .value_of("_running_version")
            .unwrap_or("noversionspecified")
    {
        info!("restarting with version {}", &parsed_config.anysnake2.rev);
        let repo = format!(
            "{}?rev={}",
            &parsed_config.anysnake2.url.as_ref().unwrap(),
            lookup_github_tag(
                &parsed_config.anysnake2.url.as_ref().unwrap(),
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

fn collect_python_packages(
    parsed_config: &mut config::ConfigToml,
) -> Result<(
    Vec<(String, String)>,
    Vec<(String, HashMap<String, String>)>,
)> {
    Ok(match &mut parsed_config.python {
        Some(python) => {
            let mut requirement_packages: Vec<(String, String)> = Vec::new();
            let mut build_packages: Vec<(String, HashMap<String, String>)> = Vec::new();
            for (name, pp) in python.packages.drain() {
                match pp {
                    PythonPackageDefinition::Requirement(str_package_definition) => {
                        requirement_packages.push((name, str_package_definition));
                    }
                    PythonPackageDefinition::BuildPythonPackage(bp_definition) => {
                        build_packages.push((name, bp_definition))
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
            (requirement_packages, build_packages)
        }
        None => (Vec::new(), Vec::new()),
    })
}

#[allow(clippy::vec_init_then_push)]
fn inner_main() -> Result<()> {
    install_ctrl_c_handler()?;
    let matches = parse_args();
    configure_logging(&matches);

    handle_config_command(&matches)?;
    let top_level_slop: Vec<&str> = match matches.values_of("slop") {
        Some(slop) => slop.collect(),
        None => Vec::new(),
    };

    let cmd = match matches.subcommand() {
        (name, Some(_subcommand)) => name,
        _ => {
            if top_level_slop.is_empty() {
                "default"
            } else {
                top_level_slop[0]
            }
        }
    };

    if std::env::var("SINGULARITY_NAME").is_ok() {
        bail!("Can't run anysnake within singularity container - nesting not supported");
    }

    let mut parsed_config: config::ConfigToml = read_config(&matches)?;

    let flake_dir: PathBuf = [".anysnake2_flake"].iter().collect();
    fs::create_dir_all(&flake_dir)?; //we must create it now, so that we can store the anysnake tag lookup

    switch_to_configured_version(&parsed_config, &matches, &flake_dir)?;
    if cmd == "version" {
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

    lookup_clones(&mut parsed_config)?;
    perform_clones(&parsed_config)?;

    let (python_packages, mut python_build_packages) = collect_python_packages(&mut parsed_config)?;
    trace!(
        "python packages: {:?} {:?}",
        python_packages,
        python_build_packages
    );
    apply_trust_on_first_use(&parsed_config, &mut python_build_packages)?;
    let use_generated_file_instead = parsed_config.anysnake2.do_not_modify_flake.unwrap_or(false);

    let flake_changed = flake_writer::write_flake(
        &flake_dir,
        &mut parsed_config,
        &python_packages,
        &python_build_packages,
        use_generated_file_instead,
    )?;

    if let ("build", Some(sc)) = matches.subcommand() {
        {
            match sc.subcommand().0 {
                "flake" => {
                    println!("Writing just flake/flake.nix");
                    rebuild_flake(use_generated_file_instead, "flake", &flake_dir)?;
                }
                "sif" => {
                    println!("Building sif in flake/result/...sif");
                    rebuild_flake(
                        use_generated_file_instead,
                        "sif_image.x86_64-linux",
                        &flake_dir,
                    )?;
                }
                "rootfs" => {
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

        let nixpkgs_url = format!(
            "{}?rev={}",
            &parsed_config.outside_nixpkgs.url,
            lookup_github_tag(
                &parsed_config.outside_nixpkgs.url,
                &parsed_config.outside_nixpkgs.rev,
                &flake_dir
            )?,
        );

        let build_output: PathBuf = flake_dir.join("result/rootfs");
        let build_unfinished_file = flake_dir.join(".build_unfinished"); // ie. the flake build failed
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

            if cmd == "run" {
                let slop = matches.subcommand().1.unwrap().values_of("slop");
                let slop: Vec<&str> = match slop {
                    Some(slop) => slop.collect(),
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
                        run_bash(bash_script).context("pre run outside failed")?;
                    }
                    None => {}
                };
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
                    let egg_target = fs::read_to_string(egg_link)?
                        .split_once("\n")
                        .context("No newline in egg-link?")?
                        .0
                        .to_string();
                    python_paths.push(egg_target)
                }
                envs.push(format!("PYTHONPATH={}", python_paths.join(":")));
                paths.push("/anysnake2/venv/bin");
            };

            match &parsed_config.container.volumes_ro {
                Some(volumes_ro) => {
                    for (from, to) in volumes_ro {
                        let from: PathBuf = fs::canonicalize(&from)
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
                        let from: PathBuf = fs::canonicalize(&from)
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
                        "An error occured when running the post_run_outside bash script: {}",
                        e
                    )
                }
            };
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

/// expand clones by clone_regeps, verify url schema

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
                    if !(proto_url.starts_with("git+") || proto_url.starts_with("hg+")) {
                        bail!("Url did not start with git+ or hg+ which are the only supported version control formats {}. (Possibly rewritten using clone_regexps", proto_url);
                    }
                }
            }
        }
        None => {}
    };
    //assert!(re.is_match("2014-01-01"));

    Ok(())
}

fn dir_empty(path: &PathBuf) -> Result<bool> {
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
                        let output = run_without_ctrl_c(|| {
                            Command::new(cmd)
                                .args(&["clone", clone_url_for_cmd, "."])
                                .current_dir(&final_dir)
                                .output()
                                .context(format!(
                                    "Failed to execute clone {target_dir}/{name} from {url}.",
                                    target_dir = target_dir,
                                    name = name,
                                    url = url
                                ))
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
                                            "Failed to execute checkout revision {} in {}",
                                            v,
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
                .args(&["commit", "-m", "autocommit"])
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
                .args(&["build", &format!("./#{}", target), "-v",
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

fn run_bash(script: &str) -> Result<()> {
    run_without_ctrl_c(|| {
        let mut child = Command::new("bash").stdin(Stdio::piped()).spawn()?;
        let child_stdin = child.stdin.as_mut().unwrap();
        child_stdin.write_all(b"set -euo pipefail\n")?;
        child_stdin.write_all(script.as_bytes())?;
        child_stdin.write_all(b"\n")?;
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
    input.replace("_", "-")
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
        if !egg_link.exists() {
            // so that changing python versions triggers a rebuild.
            to_build.push((safe_pkg, target_dir));
        }
    }
    if !to_build.is_empty() {
        for (safe_pkg, target_dir) in to_build.iter() {
            info!("Pip install {:?}", &target_dir);
            let td = tempdir::TempDir::new("anysnake_venv")?;
            let td_home = tempdir::TempDir::new("anysnake_venv")?;
            let td_home_str = td_home.path().to_string_lossy().to_string();
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
            ];
            singularity_args.push(flake_dir.join("result/rootfs").to_string_lossy());
            singularity_args.push("bash".into());
            singularity_args.push("-c".into());
            singularity_args.push(format!(
                "mkdir /tmp/venv && cd /anysnake2/venv/linked_in/{} && pip --disable-pip-version-check install -e . --prefix=/tmp/venv && (cp /tmp/venv/bin/* /anysnake2/venv/bin || true)",
                &safe_pkg
            ));
            let singularity_result = run_singularity(
                &singularity_args[..],
                outside_nixpkgs_url,
                Some(&venv_dir.join("singularity.bash")),
                None,
                &flake_dir,
            )?;
            if !singularity_result.success() {
                bail!(
                    "Singularity pip install failed with exit code {}",
                    singularity_result.code().unwrap()
                );
            }
            let target_egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
            let source_egg_link = td
                .path()
                .join("venv/lib")
                .join(format!("python{}", python_version))
                .join("site-packages")
                .join(format!("{}.egg-link", &safe_pkg));
            fs::write(target_egg_link, ex::fs::read_to_string(source_egg_link)?)?;

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

fn symlink_for_sure<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> Result<()> {
    debug!(
        "symlink_for_sure {:?} <- {:?}",
        &original.as_ref(),
        &link.as_ref()
    );
    if fs::read_link(&link).is_ok() {
        // ie it existed...
        debug!("removing old symlink {:?}", &link.as_ref());
        fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(&original, &link).with_context(|| {
        format!(
            "Failed to symlink {:?} to {:?}",
            &original.as_ref(),
            &link.as_ref()
        )
    })
}

pub fn register_nix_gc_root(url: &str, flake_dir: impl AsRef<Path>) -> Result<()> {
    debug!("registering gc root for {}", url);
    //where we store this stuff
    let gc_roots = flake_dir.as_ref().join(".gcroots");
    fs::create_dir_all(&gc_roots)?;

    //where nix goes on the hunt
    //
    let gc_per_user_base: PathBuf = ["/nix/var/nix/gcroots/per-user", &whoami::username()]
        .iter()
        .collect();
    let flake_hash = sha256::digest(
        flake_dir
            .as_ref()
            .to_owned()
            .into_os_string()
            .to_string_lossy(),
    );

    //first we store and hash the flake itself and record tha.
    let (without_hash, _) = url.rsplit_once('#').expect("GC_root url should contain #");
    let flake_symlink_here = gc_roots.join(&without_hash.replace("/", "_"));
    if !flake_symlink_here.exists() {
        debug!("nix prefetching flake {}", &without_hash);
        run_without_ctrl_c(|| {
            let output = std::process::Command::new("nix")
                .args(&["flake", "prefetch", without_hash, "--json"])
                .output()?;
            if !output.status.success() {
                Err(anyhow!("nix build failed"))
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let j: NixFlakePrefetchOutput = serde_json::from_str(&stdout)?;
                symlink_for_sure(&j.storePath, &flake_symlink_here)?;
                symlink_for_sure(
                    &gc_roots
                        .canonicalize()?
                        .join(&without_hash.replace("/", "_")),
                    &gc_per_user_base.join(&format!(
                        "{}_{}",
                        &flake_hash,
                        &without_hash.replace("/", "_")
                    )),
                )?;
                //now from the gc_dir
                Ok(())
            }
        })?;
    }

    //now the nix build.

    let out_dir = gc_roots.join(&url.replace("/", "_"));
    let rev_file = gc_roots.join(format!("{}.rev", url.replace("/", "_")));
    let last = fs::read_to_string(&rev_file).unwrap_or_else(|_| "".to_string());
    if last != url || !out_dir.exists() {
        fs::remove_file(&out_dir).ok();
        fs::write(&rev_file, &url).ok();
        debug!("nix building {}", &url);

        let store_path = run_without_ctrl_c(|| {
            let output = std::process::Command::new("nix")
                .args(&[
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
        })?;
        symlink_for_sure(store_path, &out_dir)?;
        symlink_for_sure(
            &out_dir
                .parent()
                .context("parent not found")?
                .canonicalize()?
                .join(&url.replace("/", "_")),
            &gc_per_user_base.join(&format!("{}_{}", &flake_hash, &url.replace("/", "_"))),
        )?;
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
        let real_target = parent_dir.join(&spec.strip_prefix("editable/").unwrap());
        let egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
        let egg_target = fs::read_to_string(egg_link)?
            .split_once("\n")
            .context("No newline in egg-link?")?
            .0
            .to_string();
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

fn apply_trust_on_first_use(
    config: &config::ConfigToml,
    python_build_packages: &mut Vec<(String, HashMap<String, String>)>,
) -> Result<()> {
    if !python_build_packages.is_empty() {
        use toml_edit::{value, Document};
        let toml = std::fs::read_to_string(&config.source).expect("Could not reread config file");
        let mut doc = toml.parse::<Document>().expect("invalid doc");
        let mut write = false;

        for (k, spec) in python_build_packages.iter_mut() {
            let method = spec
                .get("method")
                .expect("missing method - should have been caught earlier");
            if method == "fetchFromGitHub" {
                write = true;
                println!("Using Trust-On-First-Use for python package {}, updating your anysnake2.toml", k);

                let hash = prefetch_github_hash(
                    spec.get("owner").expect("missing owner"),
                    spec.get("repo").expect("missing repo"),
                    spec.get("rev").expect("missing rev"),
                )?;
                println!("hash is {}", hash);
                let key = k.to_owned();
                doc["python"]["packages"][key]["hash"] = value(&hash);
                spec.insert("hash".to_string(), hash.to_owned());
            }
        }
        if write {
            let out_toml = doc.to_string();
            std::fs::write("anysnake2.toml", out_toml).expect("failed to rewrite config file");
        }
    }
    Ok(())
}

fn prefetch_github_hash(owner: &str, repo: &str, git_hash: &str) -> Result<String> {
    let url = format!(
        "https://github.com/{owner}/{repo}/archive/{git_hash}.tar.gz",
        owner = owner,
        repo = repo,
        git_hash = git_hash
    );

    let old_format = Command::new("nix-prefetch-url")
        .args(&[&url, "--type", "sha256", "--unpack"])
        .output()
        .context(format!("Failed to nix-prefetch {url}", url = url))?
        .stdout;
    let old_format = std::str::from_utf8(&old_format)
        .context("nix-prefetch result was no utf8")?
        .trim();
    let new_format = convert_hash_to_subresource_format(old_format)?;
    println!("before convert: {}, after: {}", &old_format, &new_format);
    Ok(new_format)
}

fn convert_hash_to_subresource_format(hash: &str) -> Result<String> {
    let res = Command::new("nix")
        .args(&["hash", "to-sri", "--type", "sha256", hash])
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
        Err(anyhow!("nix hash to-sri returned empty result"))
    } else {
        Ok(res)
    }
}
