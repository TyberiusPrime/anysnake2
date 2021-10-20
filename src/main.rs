extern crate clap;
use anyhow::{anyhow, bail, Context, Result};
use clap::{value_t, App, AppSettings, Arg, ArgMatches, SubCommand};
use lazy_static::lazy_static;
use log::{debug, error, info, trace, warn};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/* TODO
 *
 * running container managment (does it die when you quit the shell. Yes it does.? can we reattach?
   should we just all use screen all the time?)

 * R
 * pypyi-debs that were not flakes... when is the cut off , how do we get around it 2021-04-12

*/

mod config;
mod flake_writer;
mod maps_duplicate_key_is_error;
mod python_parsing;

use flake_writer::lookup_github_tag;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let r = inner_main();
    match r {
        Err(e) => {
            error!("{:?}", e); //so the error messages are colorfull
            std::process::exit(1);
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
        .subcommand(SubCommand::with_name("version").about("output version of this build"))
        .subcommand(
            SubCommand::with_name("run")
                .about("run arbitray commands in container (w/o any pre/post bash scripts)")
                .arg(
                    Arg::with_name("slop").takes_value(true).multiple(true), //.last(true), // Indicates that `slop` is only accessible after `--`.
                ),
        )
        .get_matches()
}

fn handle_config_command(matches: &ArgMatches<'static>) {
    if let ("config", Some(sc)) = matches.subcommand() {
        {
            match sc.subcommand().0 {
                "minimal" => println!(
                    "{}",
                    std::include_str!("../examples/minimal/anysnake2.toml")
                ),
                "full" => println!("{}", std::include_str!("../examples/full/anysnake2.toml")),
                _ => {
                    // includes basic
                    println!("{}", std::include_str!("../examples/basic/anysnake2.toml"))
                }
            }
            std::process::exit(0);
        }
    }
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
    let raw_config = std::fs::read_to_string(config_file).context(format!(
        "Could not find config file {}. Use --help for help",
        config_file
    ))?;
    let parsed_config: config::ConfigToml = toml::from_str(&raw_config)
        .with_context(|| format!("Failure parsing {:?}", std::fs::canonicalize(config_file)))?;
    Ok(parsed_config)
}

fn switch_to_configured_version(
    parsed_config: &config::ConfigToml,
    matches: &ArgMatches<'static>,
) -> Result<()> {
    if parsed_config.anysnake2.rev == "dev" {
        info!("Using development version of anysnake");
    } else if parsed_config.anysnake2.rev
        != matches
            .value_of("_running_version")
            .unwrap_or("noversionspecified")
    {
        info!("restarting with version {}", &parsed_config.anysnake2.rev);
        let repo = format!(
            "{}?rev={}",
            &parsed_config.anysnake2.url,
            lookup_github_tag(&parsed_config.anysnake2.url, &parsed_config.anysnake2.rev)?
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
            let status = run_without_ctrl_c(|| Ok(Command::new("nix").args(&args).status()?))?;
            //now push
            std::process::exit(status.code().unwrap());
        }
    }
    Ok(())
}

fn collect_python_packages(
    parsed_config: &mut config::ConfigToml,
) -> Result<Vec<(String, String)>> {
    Ok(match &mut parsed_config.python {
        Some(python) => {
            let mut res: Vec<(String, String)> = python.packages.drain().collect();
            if !res.is_empty() {
                //don't need pip if we ain't got no packages (and therefore no editable packages
                res.push(("pip".into(), "".into())); // we use pip to build editable packages
                res.push(("setuptools".into(), "".into())); // we use pip to build editable packages
            }
            match &parsed_config.clones {
                Some(clones) => {
                    let python_requirements_from_clones =
                        python_parsing::find_python_requirements_for_clones(clones)?;
                    for (pkg, version_spec) in python_requirements_from_clones.into_iter() {
                        res.push((pkg, version_spec));
                    }
                }
                None => {}
            };
            res
        }
        None => Vec::new(),
    })
}

#[allow(clippy::vec_init_then_push)]
fn inner_main() -> Result<()> {
    install_ctrl_c_handler()?;
    let matches = parse_args();

    handle_config_command(&matches);

    let cmd = match matches.subcommand() {
        (name, Some(_subcommand)) => name,
        _ => "default",
    };

    configure_logging(&matches);

    if cmd == "version" {
        print_version_and_exit();
    }

    let mut parsed_config: config::ConfigToml = read_config(&matches)?;

    let flake_dir: PathBuf = ["flake"].iter().collect();
    std::fs::create_dir_all(&flake_dir)?; //we must create it now, so that we can store the anysnake tag lookup

    switch_to_configured_version(&parsed_config, &matches)?;

    if !(parsed_config.cmd.contains_key(cmd) || cmd == "build" || cmd == "run") {
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

    let python_packages = collect_python_packages(&mut parsed_config)?;
    trace!("python packages: {:?}", python_packages);
    let use_generated_file_instead = parsed_config.anysnake2.do_not_modify_flake.unwrap_or(false);

    let flake_changed = flake_writer::write_flake(
        &flake_dir,
        &parsed_config,
        &python_packages,
        use_generated_file_instead,
    )?;

    if let ("build", Some(sc)) = matches.subcommand() {
        {
            match sc.subcommand().0 {
                "sif" => {
                    println!("Building sif in flake/result/...sif");
                    rebuild_flake(use_generated_file_instead, "sif_image.x86_64-linux")?;
                }
                "rootfs" => {
                    println!("Building rootfs in flake/result");
                    rebuild_flake(use_generated_file_instead, "")?;
                }
                _ => {
                    println!("Please pass a subcommand as to what to build");
                    std::process::exit(1);
                }
            }
        }
    } else {
        let build_output: PathBuf = ["flake", "result", "rootfs"].iter().collect();
        let build_unfinished_file = flake_dir.join(".build_unfinished"); // ie. the flake build failed
        if flake_changed || !build_output.exists() || build_unfinished_file.exists() {
            info!("Rebuilding flake");
            rebuild_flake(use_generated_file_instead, "")?;
        }

        let nixpkgs_url = format!(
            "{}?rev={}",
            &parsed_config.nixpkgs.url,
            lookup_github_tag(&parsed_config.nixpkgs.url, &parsed_config.nixpkgs.rev)?,
        );

        if let Some(python) = &parsed_config.python {
            fill_venv(&python.version, &python_packages, &nixpkgs_url)?;
        };

        let home_dir = PathBuf::from(replace_env_vars(
            parsed_config.container.home.as_deref().unwrap_or("$HOME"),
        ));
        let home_dir_str: String = home_dir
            .clone()
            .into_os_string()
            .to_string_lossy()
            .to_string();
        debug!("Using {:?} as home", home_dir);
        std::fs::create_dir_all(home_dir).context("Failed to create home dir")?;

        let run_dir: PathBuf = ["flake/run_scripts/", cmd].iter().collect();
        let outer_run_sh: PathBuf = run_dir.join("outer_run.sh");
        let run_sh: PathBuf = run_dir.join("run.sh");
        std::fs::create_dir_all(&run_dir).context("Failed to create run dir for scripts")?;
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
            std::fs::write(&outer_run_sh, "#/bin/bash\nbash /anysnake2/run.sh\n")?;
            std::fs::write(&run_sh, slop.join(" "))?;
            std::fs::write(&post_run_sh, "")?;
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
            let post_run_script =
                run_template.replace("%RUN%", cmd_info.post_run_inside.as_deref().unwrap_or(""));
            std::fs::write(
                &outer_run_sh,
                "#/bin/bash\nbash /anysnake2/run.sh\nexport ANYSNAKE_RUN_STATUS=$?\nbash /anysnake2/post_run.sh",
            )?;
            std::fs::write(&run_sh, run_script)?;
            std::fs::write(&post_run_sh, post_run_script)?;
            post_run_outside = cmd_info.post_run_outside.clone();
        }

        let outer_run_sh_str: String = outer_run_sh.into_os_string().to_string_lossy().to_string();
        let run_sh_str: String = run_sh.into_os_string().to_string_lossy().to_string();
        let post_run_sh_str: String = post_run_sh.into_os_string().to_string_lossy().to_string();

        let mut singularity_args: Vec<String> = vec![
            "exec".into(),
            "--userns".into(),
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
            let venv_dir: PathBuf = ["venv", &python.version].iter().collect();
            binds.push((
                format!("venv/{}", python.version),
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
                    target_dir.into_os_string().to_string_lossy().to_string(),
                    format!("/anysnake2/venv/linked_in/{}", safe_pkg),
                    "ro".to_string(),
                ));
                let egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
                let egg_target = std::fs::read_to_string(egg_link)?
                    .split_once("\n")
                    .context("No newline in egg-link?")?
                    .0
                    .to_string();
                python_paths.push(egg_target)
            }
            envs.push(format!("PYTHONPATH={}", python_paths.join(":")));
        };

        match &parsed_config.container.volumes_ro {
            Some(volumes_ro) => {
                for (from, to) in volumes_ro {
                    let from: PathBuf =
                        std::fs::canonicalize(&from).context(format!("abs_path on {}", &from))?;
                    let from = from.into_os_string().to_string_lossy().to_string();
                    binds.push((from, to.to_string(), "ro".to_string()));
                }
            }
            None => {}
        };
        match &parsed_config.container.volumes_rw {
            Some(volumes_ro) => {
                for (from, to) in volumes_ro {
                    let from: PathBuf =
                        std::fs::canonicalize(&from).context(format!("abs_path on {}", &from))?;
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
                //std::fs::canonicalize(from)?
                //.into_os_string()
                //.to_string_lossy(),
                from,
                to,
                opts
            ));
        }

        if let Some(container_envs) = &parsed_config.container.env {
            for (k, v) in container_envs.iter() {
                envs.push(format!("{}={}", k, v));
            }
        }

        for e in envs.into_iter() {
            singularity_args.push("--env".into());
            singularity_args.push(e);
        }

        singularity_args.push("flake/result/rootfs".into());
        singularity_args.push("/bin/bash".into());
        singularity_args.push("/anysnake2/outer_run.sh".into());
        let singularity_result = run_singularity(
            &singularity_args[..],
            &nixpkgs_url,
            Some(&run_dir.join("singularity.bash")),
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
    nix_repo: &str,
    log_file: Option<&PathBuf>,
) -> Result<std::process::ExitStatus> {
    run_without_ctrl_c(|| {
        let mut full_args = vec![
            "shell".to_string(),
            format!("{}#singularity", nix_repo),
            "-c".into(),
            "singularity".into(),
        ];
        for arg in args {
            full_args.push(arg.to_string());
        }
        let pp = pretty_print_singularity_call(&full_args);
        if let Some(lf) = log_file {
            let o = format!("nix {}", pp.trim_start());
            std::fs::write(lf, o)?;
        }
        info!("nix {}", pp.trim_start());
        Ok(Command::new("nix").args(full_args).status()?)
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
        res += arg;
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
                            if !(out.starts_with("git+") || out.starts_with("hg+")) {
                                bail!("Url did not start with git+ or hg+ which are the only supported version control formats {}=>{}", proto_url, out);
                            }
                            *proto_url = out; // know it's the real url
                        }
                    }
                }
            }
        }
        None => {}
    };
    //assert!(re.is_match("2014-01-01"));

    Ok(())
}

fn perform_clones(parsed_config: &config::ConfigToml) -> Result<()> {
    match &parsed_config.clones {
        Some(clones) => {
            for (target_dir, name_urls) in clones.iter() {
                std::fs::create_dir_all(target_dir)
                    .context(format!("Could not create {}", target_dir))?;
                let clone_log: PathBuf = [target_dir, ".clone_info.json"].iter().collect();
                let mut known_clones: HashMap<String, String> = match clone_log.exists() {
                    true => serde_json::from_str(&std::fs::read_to_string(&clone_log)?)?,
                    false => HashMap::new(),
                };
                for (name, url) in name_urls {
                    let known_url = match known_clones.get(name) {
                        Some(x) => x,
                        None => "",
                    };
                    let final_dir: PathBuf = [target_dir, name].iter().collect();
                    if known_url != url && final_dir.exists() {
                        let msg = format!(
                            "Url changed for clone target: {target_dir}/{name}. Was '{known_url}' is now '{url}'.\n\
                        Cowardly refusing to throw away old checkout."
                        , target_dir=target_dir, name=name, known_url=known_url, url=url);
                        bail!(msg);
                    }
                }
                for (name, url) in name_urls {
                    let final_dir: PathBuf = [target_dir, name].iter().collect();
                    std::fs::create_dir_all(&final_dir)?;
                    let is_empty = final_dir.read_dir()?.next().is_none();
                    if is_empty {
                        info!("cloning {}/{} from {}", target_dir, name, url);
                        known_clones.insert(name.clone(), url.clone());
                        let (cmd, furl) = if url.starts_with("git+") {
                            ("git", url.strip_prefix("git+").unwrap())
                        } else if url.starts_with("hg+") {
                            ("hg", url.strip_prefix("hg+").unwrap())
                        } else {
                            bail!("Unexpected url schema - should have been tested before");
                        };
                        let output = run_without_ctrl_c(|| {
                            Command::new(cmd)
                                .args(&["clone", furl, "."])
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
                    }
                }
                std::fs::write(
                    clone_log,
                    serde_json::to_string_pretty(&json!(known_clones))?,
                )?;
            }
        }
        None => {}
    };

    Ok(())
}

fn rebuild_flake(use_generated_file_instead: bool, target: &str) -> Result<()> {
    let flake_dir: PathBuf = ["flake"].iter().collect();
    std::fs::write(
        flake_dir.join(".gitignore"),
        "result
run_scripts/
.*.json
",
    )?;
    debug!("writing flake");
    let mut gitargs = vec!["add", "flake.nix", ".gitignore"];
    if flake_dir.join("flake.lock").exists() {
        gitargs.push("flake.nix");
    }

    let output = run_without_ctrl_c(|| {
        Command::new("git")
            .args(&gitargs)
            .current_dir(&flake_dir)
            .output()
            .context("Failed git add flake.nix")
    })?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!(
            "Failed git add flake.nix. \n Stdout {:?}\nStderr: {:?}",
            stdout, stderr
        );
        bail!(msg);
    }

    if !use_generated_file_instead {
        run_without_ctrl_c(|| {
            Command::new("git")
                .args(&["commit", "-m", "autocommit"])
                .current_dir(&flake_dir)
                .output()
                .context("Failed git add flake.nix")
        })?;
    }
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!(
            "Failed git commit flake.nix. \n Stdout {:?}\nStderr: {:?}",
            stdout, stderr
        );
        bail!(msg);
    }
    let build_unfinished_file = flake_dir.join(".build_unfinished");
    std::fs::write(&build_unfinished_file, "in_progress")?;

    let nix_build_result = run_without_ctrl_c(|| {
        Ok(Command::new("nix")
            .args(&["build", &format!("./#{}", target), "-v", "--show-trace"])
            .current_dir("flake")
            .status()?)
    })?;
    if nix_build_result.success() {
        std::fs::remove_file(&build_unfinished_file)?;
        Ok(())
    } else {
        Err(anyhow!("flake building failed"))
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
    nixpkgs_url: &str, //clones: &HashMap<String, HashMap<String, String>>, //target_dir, name, url
) -> Result<()> {
    let venv_dir: PathBuf = ["venv", python_version].iter().collect();
    std::fs::create_dir_all(&venv_dir)?;
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
            let mut singularity_args: Vec<String> = vec![
                "exec".into(),
                "--userns".into(),
                "--no-home".into(),
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
            ];
            singularity_args.push("flake/result/rootfs".into());
            singularity_args.push("bash".into());
            singularity_args.push("-c".into());
            singularity_args.push(format!(
                "mkdir /tmp/venv && cd /anysnake2/venv/linked_in/{} && pip --disable-pip-version-check install -e . --prefix=/tmp/venv",
                &safe_pkg
            ));
            let singularity_result = run_singularity(
                &singularity_args[..],
                nixpkgs_url,
                Some(&venv_dir.join("singularity.bash")),
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
            std::fs::write(target_egg_link, std::fs::read_to_string(source_egg_link)?)?;

            /*keep it here in case we need it again...
             * for dir_entry in walkdir::WalkDir::new(td.path()) {
                let dir_entry = dir_entry?;
                if let Some(filename) = dir_entry.file_name().to_str() {
                    if filename.ends_with(".egg-link") {
                        trace!("found {:?} for {:?}", &safe_pkg, &dir_entry);
                        std::fs::write(
                            target_egg_link,
                            std::fs::read_to_string(dir_entry.path())?,
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
