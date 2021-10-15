extern crate clap;
use anyhow::{anyhow, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use clap::{App, AppSettings, Arg, SubCommand};
use fstrings::{format_args_f, format_f, println_f};
use regex::Regex;
use serde_derive::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::io::{self, BufRead};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/* TODO
 *
 * running container managment (does it die when you quit the shell? can we reattach? should we
   just all use screen all the time?)

 * R

 * sensible verbosity...
 *
 * Get rid of walkdir
 * refactor

*/

const VERSION: &str = env!("CARGO_PKG_VERSION");

trait WithDefaultFlakeSource {
    fn default_rev() -> String;
    fn default_url() -> String;
}

#[derive(Deserialize, Debug)]
struct ConfigToml {
    anysnake2: Anysnake2,
    nixpkgs: NixPkgs,
    outside_nixpkgs: NixPkgs,
    #[serde(default, rename = "flake-util")]
    flake_util: FlakeUtil,
    clone_regexps: Option<HashMap<String, String>>,
    clones: Option<HashMap<String, HashMap<String, String>>>,
    #[serde(default)]
    cmd: HashMap<String, Cmd>,
    #[serde(default)]
    rust: Rust,
    python: Option<Python>,
    #[serde(default, rename = "mach-nix")]
    mach_nix: MachNix,
    container: Option<Container>,
    flakes: Option<HashMap<String, Flake>>,
}
#[derive(Deserialize, Debug)]
struct Anysnake2 {
    rev: String,
    #[serde(default = "Anysnake2::default_url")]
    url: String,
    do_not_modify_flake: Option<bool>,
}

impl Anysnake2 {
    fn default_url() -> String {
        "github:TyberiusPrime/anysnake2".to_string()
    }
}

#[derive(Deserialize, Debug)]
struct NixPkgs {
    rev: String,
    #[serde(default = "NixPkgs::default_url")]
    url: String,
    packages: Option<Vec<String>>,
}
impl NixPkgs {
    fn default_url() -> String {
        "github:NixOS/nixpkgs".to_string()
    }
}

#[derive(Deserialize, Debug)]
struct FlakeUtil {
    #[serde(default = "FlakeUtil::default_rev")]
    rev: String,
    #[serde(default = "FlakeUtil::default_url")]
    url: String,
}

impl WithDefaultFlakeSource for FlakeUtil {
    fn default_rev() -> String {
        "7e5bf3925f6fbdfaf50a2a7ca0be2879c4261d19".to_string()
    }

    fn default_url() -> String {
        "github:numtide/flake-utils".to_string()
    }
}

impl Default for FlakeUtil {
    fn default() -> Self {
        FlakeUtil {
            rev: Self::default_rev(),
            url: Self::default_url(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct Cmd {
    run: String,
    pre_run_outside: Option<String>,
    post_run_inside: Option<String>,
    post_run_outside: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Rust {
    version: Option<String>,
    #[serde(default = "Rust::default_rev")]
    rust_overlay_rev: String,
    #[serde(default = "Rust::default_url")]
    rust_overlay_url: String,
}

impl Default for Rust {
    fn default() -> Self {
        Rust {
            version: None,
            rust_overlay_rev: Self::default_rev(),
            rust_overlay_url: Self::default_url(),
        }
    }
}

impl WithDefaultFlakeSource for Rust {
    fn default_rev() -> String {
        "08de2ff90cc08e7f9523ad97e4c1653b09f703ec".to_string()
    }
    fn default_url() -> String {
        "github:oxalica/rust-overlay".to_string()
    }
}

#[derive(Deserialize, Debug)]
struct Python {
    version: String,
    //#[serde(with = "my_date_format")]
    //ecosystem_date: DateTime<Utc>,
    ecosystem_date: String,
    #[serde(with = "serde_with::rust::maps_duplicate_key_is_error")]
    packages: HashMap<String, String>,
}
#[derive(Deserialize, Debug)]
struct MachNix {
    #[serde(default = "MachNix::default_rev")]
    rev: String,
    #[serde(default = "MachNix::default_url")]
    url: String,
}

impl Default for MachNix {
    fn default() -> Self {
        MachNix {
            rev: Self::default_rev(),
            url: Self::default_url(),
        }
    }
}

impl WithDefaultFlakeSource for MachNix {
    fn default_rev() -> String {
        "3.3.0".to_string()
    }
    fn default_url() -> String {
        "github:DavHau/mach-nix".to_string()
    }
}

#[derive(Deserialize, Debug)]
struct Flake {
    url: String,
    rev: String,
    follows: Option<Vec<String>>,
    packages: Vec<String>,
}
#[derive(Deserialize, Debug)]
struct Container {
    home: Option<String>,
    volumes_ro: Option<HashMap<String, String>>,
    volumes_rw: Option<HashMap<String, String>>,
}

fn parse_my_date(s: &str) -> Result<chrono::NaiveDate> {
    const FORMAT: &str = "%Y-%m-%d %H:%M:%S";
    use chrono::TimeZone;
    Ok(chrono::Utc
        .datetime_from_str(&format!("{} 00:00:00", s), FORMAT)?
        .naive_utc()
        .date())
}

struct InputFlake {
    name: String,
    url: String,
    rev: String,
    follows: Vec<String>,
}

impl InputFlake {
    fn new(name: &str, url: &str, rev: &str, follows: &[&str]) -> Result<Self> {
        let url = if url.ends_with("/") {
            url.strip_suffix("/").unwrap()
        } else {
            url
        };
        Ok(InputFlake {
            name: name.to_string(),
            url: url.to_string(),
            rev: lookup_github_tag(url, rev)?,
            follows: follows.iter().map(|x| x.to_string()).collect(),
        })
    }
}

fn main() -> Result<()> {
    let matches = App::new("Anysnake2")
        .version("0.1")
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
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity (can be passed up to three times)"),
        )
        .arg(
            Arg::with_name("_running_version")
                .long("_running_version")
                .help("internal use only")
                .hidden(true)
                .takes_value(true),
        )
        .subcommand(
            SubCommand::with_name("build").about("build container, but do not run anything"),
        )
        .subcommand(
            SubCommand::with_name("example-config")
                .about("dump an example anysnake2.toml to stdout"),
        )
        .subcommand(SubCommand::with_name("version").about("output version of this build"))
        .get_matches();

    let cmd = match matches.subcommand() {
        (name, Some(_subcommand)) => name,
        _ => "default",
    };

    if cmd == "example-config" {
        println!("# dump this to anysnake2.toml (default filename)");
        println!("{}", std::include_str!("../examples/full/anysnake2.toml"));
        std::process::exit(0);
    }
    let config_file = matches.value_of("config_file").unwrap_or("anysnake2.toml");
    let config_file_path: PathBuf = [config_file].iter().collect();
    if !config_file_path.exists() && cmd == "version" {
        print_version();
    }

    let raw_config = std::fs::read_to_string(config_file).context(format!(
        "Could not find config file {}. Use --help for help",
        config_file
    ))?;
    let mut parsed_config: ConfigToml = toml::from_str(&raw_config)
        .with_context(|| format!("Failure parsing {:?}", std::fs::canonicalize(config_file)))?;
    let flake_dir: PathBuf = ["flake"].iter().collect();
    std::fs::create_dir_all(&flake_dir)?; //we must create it now, so that we can store the anysnake tag lookup

    if parsed_config.anysnake2.rev == "dev" {
        println!("Using development version of anysnake");
    } else if parsed_config.anysnake2.rev
        != matches
            .value_of("_running_version")
            .unwrap_or("noversionspecified")
    {
        println!("restarting with version {}", &parsed_config.anysnake2.rev);
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
            println!("new args {:?}", args);
            let status = Command::new("nix").args(args).status()?;
            //now push
            std::process::exit(status.code().unwrap());
        }
    }
    if cmd == "version" {
        print_version();
    }

    if !parsed_config.cmd.contains_key(cmd) && cmd != "build" {
        return Err(anyhow!(
            "Cmd {} not found. Available: {:?}",
            cmd,
            parsed_config.cmd.keys()
        ));
    }

    lookup_clones(&mut parsed_config)?;
    perform_clones(&parsed_config)?;

    let python_packages: Vec<(String, String)> = {
        match &mut parsed_config.python {
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
                            find_python_requirements_for_clones(clones)?;
                        for (pkg, version_spec) in python_requirements_from_clones.into_iter() {
                            res.push((pkg, version_spec));
                        }
                    }
                    None => {}
                };
                res
            }
            None => Vec::new(),
        }
    };
    println!("python packages: {:?}", python_packages);
    let use_generated_file_instead = parsed_config.anysnake2.do_not_modify_flake.unwrap_or(false);

    let nixpkgs_url = format!(
        "{}?rev={}",
        &parsed_config.nixpkgs.url,
        lookup_github_tag(&parsed_config.nixpkgs.url, &parsed_config.nixpkgs.rev)?,
    );

    let flake_changed = write_flake(
        &flake_dir,
        &parsed_config,
        &python_packages,
        use_generated_file_instead,
    )?;
    let build_output: PathBuf = ["flake", "result"].iter().collect();
    let build_unfinished_file = flake_dir.join(".build_unfinished"); // ie. the flake build failed
    if flake_changed || !build_output.exists() || build_unfinished_file.exists() {
        println!("Rebuilding");
        rebuild_flake(use_generated_file_instead)?;
    }

    match &parsed_config.python {
        Some(python) => {
            fill_venv(&python.version, &python_packages, &nixpkgs_url)?;
        }
        None => {}
    };

    let home_dir: PathBuf = {
        [replace_env_vars(
            ((|| parsed_config.container.as_ref()?.home.as_deref())()).unwrap_or("$HOME"),
        )]
        .iter()
        .collect()
    };
    let home_dir_str: String = home_dir
        .clone()
        .into_os_string()
        .to_string_lossy()
        .to_string();
    println_f!("Using {home_dir:?} as home");
    std::fs::create_dir_all(home_dir).context("Failed to create home dir")?;

    if cmd == "build" {
        println_f!("Build only - done");
    } else {
        println_f!("Running singularity - cmd {cmd}");
        let cmd_info = parsed_config.cmd.get(cmd).context("Command not found")?;
        match &cmd_info.pre_run_outside {
            Some(bash_script) => {
                run_bash(bash_script).context("pre run outside failed")?;
            }
            None => {}
        };
        let run_template = std::include_str!("run.sh");
        let run_dir = format!("flake/run_scripts/{}", cmd);
        std::fs::create_dir_all(&run_dir).context("Failed to create run dir for scripts")?;
        let run_script = run_template.replace("%RUN%", &cmd_info.run);
        let post_run_script =
            run_template.replace("%RUN%", cmd_info.post_run_inside.as_deref().unwrap_or(""));

        let outer_run_sh: PathBuf = [&run_dir, "outer_run.sh"].iter().collect(); //todo: tempfile
        let outer_run_sh_str: String = outer_run_sh
            .clone()
            .into_os_string()
            .to_string_lossy()
            .to_string();
        let run_sh: PathBuf = [&run_dir, "run.sh"].iter().collect(); //todo: tempfile
        let run_sh_str: String = run_sh
            .clone()
            .into_os_string()
            .to_string_lossy()
            .to_string();
        let post_run_sh: PathBuf = [&run_dir, "post_run.sh"].iter().collect(); //todo: tempfile
        let post_run_sh_str: String = post_run_sh
            .clone()
            .into_os_string()
            .to_string_lossy()
            .to_string();
        std::fs::write(
            &outer_run_sh,
            "#/bin/bash\nbash /anysnake2/run.sh\nexport ANYSNAKE_RUN_STATUS=$?\nbash /anysnake2/post_run.sh",
        )?;
        std::fs::write(&run_sh, run_script)?;
        std::fs::write(&post_run_sh, post_run_script)?;

        let mut singularity_args: Vec<String> = vec![
            "exec".into(),
            "--userns".into(),
            "--home".into(),
            home_dir_str,
        ];
        let mut binds = Vec::new();
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

        match &parsed_config.container {
            Some(container) => {
                match &container.volumes_ro {
                    Some(volumes_ro) => {
                        for (from, to) in volumes_ro {
                            let from: PathBuf = std::fs::canonicalize(&from)
                                .context(format!("abs_path on {}", &from))?;
                            let from = from.into_os_string().to_string_lossy().to_string();
                            binds.push((from, to.to_string(), "ro".to_string()));
                        }
                    }
                    None => {}
                };
                match &container.volumes_rw {
                    Some(volumes_ro) => {
                        for (from, to) in volumes_ro {
                            let from: PathBuf = std::fs::canonicalize(&from)
                                .context(format!("abs_path on {}", &from))?;
                            let from = from.into_os_string().to_string_lossy().to_string();
                            binds.push((from, to.to_string(), "rw".to_string()));
                        }
                    }
                    None => {}
                }
            }
            None => {}
        };
        for (from, to, opts) in binds {
            singularity_args.push("--bind".into());
            singularity_args.push(format!("{}:{}:{}", from, to, opts));
        }

        for e in envs.into_iter() {
            singularity_args.push("--env".into());
            singularity_args.push(e);
        }

        singularity_args.push("flake/result/rootfs".into());
        singularity_args.push("/bin/bash".into());
        singularity_args.push("/anysnake2/outer_run.sh".into());
        let singularity_result = run_singularity(&singularity_args[..], &nixpkgs_url)?;
        match &cmd_info.post_run_outside {
            Some(bash_script) => match run_bash(bash_script) {
                Ok(()) => {}
                Err(e) => {
                    println!("Warning: an error occured when running the post_run_outside bash script: {}", e)
                }
            },
            None => {}
        };
        std::process::exit(
            singularity_result
                .code()
                .context("No exit code inside container?")?,
        );
    }

    Ok(())
}

fn run_singularity(args: &[String], nix_repo: &str) -> Result<std::process::ExitStatus> {
    let mut full_args = vec![
        "shell".to_string(),
        format!("{}#singularity", nix_repo),
        "-c".into(),
        "singularity".into(),
    ];
    pretty_print_singularity_call(args);
    for arg in args {
        full_args.push(arg.to_string());
    }
    Ok(Command::new("nix").args(full_args).status()?)
}

fn nix_format(input: &str, nixpkgs_url: &str, nixpkgs_rev: &str) -> Result<String> {
    let full_args = vec![
        "shell".to_string(),
        format!("{}?rev={}#nixfmt", nixpkgs_url, nixpkgs_rev),
        "-c".into(),
        "nixfmt".into(),
    ];
    let mut child = Command::new("nix")
        .args(full_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let child_stdin = child.stdin.as_mut().unwrap();
    child_stdin.write_all(input.as_bytes())?;
    let out = child
        .wait_with_output()
        .context("Failed to wait on nixfmt")?; // closes stdin
    if out.status.success() {
        Ok((std::str::from_utf8(&out.stdout).context("nixfmt output wan't utf8")?).to_string())
    } else {
        Err(anyhow!(
            "nix fmt error return{}",
            out.status.code().unwrap()
        ))
    }
}

fn print_version() -> ! {
    println!("anysnake2 version: {}", VERSION);
    std::process::exit(0);
}

fn pretty_print_singularity_call(args: &[String]) -> String {
    let mut res = "  singularity \\\n".to_string();
    let mut skip_space = false;
    for arg in args.iter() {
        if skip_space {
            skip_space = false
        } else {
            res += "    ";
        }
        res += arg;
        if !(arg == "--bind" || arg == "--env" || arg == "--home") {
            res += " \\\n";
        } else {
            skip_space = true;
            res += " ";
        }
    }
    res += "\n";
    res
}

/// expand clones by clone_regeps, verify url schema

fn lookup_clones(parsed_config: &mut ConfigToml) -> Result<()> {
    let clone_regexps: Vec<(Regex, &String)> = match &parsed_config.clone_regexps {
        Some(replacements) => {
            let mut res = Vec::new();
            for (from, to) in replacements {
                let r = Regex::new(&format!("^{}$", from))
                    .context(format_f!("failed to parse {from}"))?;
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
                                return Err(anyhow!("Url did not start with git+ or hg+ which are the only supported version control formats {}=>{}", proto_url, out));
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

fn perform_clones(parsed_config: &ConfigToml) -> Result<()> {
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
                        let msg = format_f!(
                            "Url changed for clone target: {target_dir}/{name}. Was '{known_url}' is now '{url}'.\n\
                        Cowardly refusing to throw away old checkout."
                        );
                        return Err(anyhow!(msg));
                    }
                }
                for (name, url) in name_urls {
                    let final_dir: PathBuf = [target_dir, name].iter().collect();
                    std::fs::create_dir_all(&final_dir)?;
                    let is_empty = final_dir.read_dir()?.next().is_none();
                    if is_empty {
                        println_f!("cloning {target_dir}/{name} from {url}");
                        known_clones.insert(name.clone(), url.clone());
                        let (cmd, furl) = if url.starts_with("git+") {
                            ("git", url.strip_prefix("git+").unwrap())
                        } else if url.starts_with("hg+") {
                            ("hg", url.strip_prefix("hg+").unwrap())
                        } else {
                            return Err(anyhow!(
                                "Unexpected url schema - should have been tested before"
                            ));
                        };
                        let output = Command::new(cmd)
                            .args(["clone", furl, "."])
                            .current_dir(final_dir)
                            .output()
                            .context(format_f!(
                                "Failed to execute clone {target_dir}/{name} from {url}."
                            ))?;
                        if !output.status.success() {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let msg = format_f!(
                                "Failed to clone {target_dir}/{name} from {url}.\
                                                \n Stdout {stdout:?}\nStderr: {stderr:?}"
                            );
                            return Err(anyhow!(msg));
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

// The output is wrapped in a Result to allow matching on errors
// Returns an Iterator to the Reader of the lines of the file.
fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}

fn find_python_requirements_for_clones(
    clones: &HashMap<String, HashMap<String, String>>,
) -> Result<Vec<(String, String)>> {
    let mut res = HashSet::new();
    for (target_dir, name_urls) in clones.iter() {
        for (name, _url) in name_urls.iter() {
            let requirement_file: PathBuf = [target_dir, name, "requirements.txt"].iter().collect();
            if requirement_file.exists() {
                for line in read_lines(requirement_file)?
                    .map(|line| line.unwrap_or_else(|_| "".to_string()))
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                {
                    res.insert(line);
                }
            }

            let setup_cfg_file: PathBuf = [target_dir, name, "setup.cfg"].iter().collect();
            println!("looking for {:?}", &setup_cfg_file);
            if setup_cfg_file.exists() {
                let reqs = parse_python_config_file(&setup_cfg_file);
                match reqs {
                    Err(e) => {
                        println!("Warning: failed to parse {:?}: {}", setup_cfg_file, e)
                    }
                    Ok(mut reqs) => {
                        println!("requirements {:?}", reqs);
                        for k in reqs.drain(..) {
                            res.insert(k); // identical lines!
                        }
                    }
                };
            }
        }
    }
    Ok(res.into_iter().map(parse_python_package_spec).collect())
}

fn parse_python_package_spec(spec_line: String) -> (String, String) {
    let pos = spec_line.find(&['>', '<', '=', '!'][..]);
    match pos {
        Some(pos) => {
            let (name, spec) = spec_line.split_at(pos);
            (name.to_string(), spec.to_string())
        }
        None => (spec_line, "".to_string()),
    }
}

fn parse_python_config_file(setup_cfg_file: &Path) -> Result<Vec<String>> {
    //configparser does not do multi line values
    //ini dies on them as well.
    //so we do our own poor man's parsing
    println!("Parsing {:?}", &setup_cfg_file);
    let raw = std::fs::read_to_string(&setup_cfg_file)?;
    let mut res = Vec::new();
    match raw.find("[options]") {
        Some(options_start) => {
            let mut inside_value = false;
            let mut value_indention = 0;
            let mut value = "".to_string();
            for line in raw[options_start..].split('\n') {
                if !inside_value {
                    if line.contains("install_requires") {
                        let wo_indent_len = (line.replace("\t", "    ").trim_start()).len();
                        value_indention = line.len() - wo_indent_len;
                        match line.find('=') {
                            Some(equal_pos) => {
                                let v = line[equal_pos + 1..].trim_end();
                                value += v;
                                value += "\n";
                                inside_value = true;
                            }
                            None => return Err(anyhow!("No = in install_requires line")),
                        }
                    }
                } else {
                    // inside value
                    let wo_indent_len = (line.replace("\t", "    ").trim_start()).len();
                    let indent = line.len() - wo_indent_len;
                    if indent > value_indention {
                        value += line.trim_start();
                        value += "\n"
                    } else {
                        break;
                    }
                }
            }
            for line in value.split('\n') {
                if !line.trim().is_empty() {
                    res.push(line.trim().to_string())
                }
            }
        }
        None => return Err(anyhow!("no [options] in setup.cfg")),
    };
    Ok(res)
    //Err(anyhow!("Could not parse"))
}

fn write_flake(
    flake_dir: &Path,
    parsed_config: &ConfigToml,
    python_packages: &[(String, String)],
    use_generated_file_instead: bool,
) -> Result<bool> {
    let template = std::include_str!("flake_template.nix");
    let flake_filename: PathBuf = if use_generated_file_instead {
        ["flake", "flake.generated.nix"].iter().collect()
    } else {
        ["flake", "flake.nix"].iter().collect()
    };
    let old_flake_contents = {
        if flake_filename.exists() {
            std::fs::read_to_string(&flake_filename)?
        } else {
            "".to_string()
        }
    };
    let mut flake_contents: String = template.to_string();
    let mut inputs: Vec<InputFlake> = Vec::new();

    inputs.push(InputFlake::new(
        "nixpkgs",
        &parsed_config.nixpkgs.url,
        &parsed_config.nixpkgs.rev,
        &[],
    )?);
    flake_contents = match &parsed_config.nixpkgs.packages {
        Some(pkgs) => {
            let pkgs: String = pkgs
                .iter()
                .map(|x| format!("${{{}}}\n", x))
                .collect::<Vec<String>>()
                .join("\n");
            flake_contents.replace("%NIXPKGS_PACKAGES%", &pkgs)
        }
        None => flake_contents,
    };

    inputs.push(InputFlake::new(
        "flake-utils",
        &parsed_config.flake_util.url,
        &parsed_config.flake_util.rev,
        &["nixpkgs"],
    )?);

    flake_contents = match &parsed_config.rust.version {
        Some(version) => {
            inputs.push(InputFlake::new(
                "rust-overlay",
                &parsed_config.rust.rust_overlay_url,
                &parsed_config.rust.rust_overlay_rev,
                &["nixpkgs", "flake-utils"],
            )?);
            flake_contents.replace("\"%RUST%\"", &format!("pkgs.rust-bin.stable.\"{}\".minimal.override {{ extensions = [ \"rustfmt\" \"clippy\"]; }}", version))
        }
        None => flake_contents.replace("\"%RUST%\"", "null"),
    };

    flake_contents = match &parsed_config.python {
        Some(python) => {
            if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                return Err(anyhow!(
                        format!("Python version must be x.y (not x.y.z ,z is given by nixpkgs version). Was '{}'", &python.version)));
            }
            let python_major_minor = format!("python{}", python.version.replace(".", ""));

            let mut out_python_packages = extract_non_editable_python_packages(python_packages)?;
            out_python_packages.sort();
            let out_python_packages = out_python_packages.join("\n");

            let ecosystem_date = parse_my_date(&python.ecosystem_date)
                .context("Failed to parse python.ecosystem-date")?;
            let pypi_debs_db_rev = pypi_deps_date_to_rev(ecosystem_date)?;

            inputs.push(InputFlake::new(
                "mach-nix",
                &parsed_config.mach_nix.url,
                &parsed_config.mach_nix.rev,
                &["nipkgs", "flake-utils", "pypi-deps-db"],
            )?);

            inputs.push(InputFlake::new(
                "pypi-deps-db",
                "github:DavHau/pypi-deps-db",
                &pypi_debs_db_rev,
                &["nipkgs", "mach-nix"],
            )?);

            flake_contents
                .replace("%PYTHON_MAJOR_MINOR%", &python_major_minor)
                .replace("%PYTHON_PACKAGES%", &out_python_packages)
                .replace("%PYPI_DEPS_DB_REV%", &pypi_debs_db_rev)
        }
        None => flake_contents,
    };

    flake_contents = match &parsed_config.flakes {
        Some(flakes) => {
            let mut flake_packages = "".to_string();
            for (name, flake) in flakes.iter() {
                let rev_follows: Vec<&str> = match &flake.follows {
                    Some(f) => f.iter().map(|x| &x[..]).collect(),
                    None => Vec::new(),
                };
                inputs.push(InputFlake::new(
                    &name,
                    &flake.url,
                    &flake.rev,
                    &rev_follows[..],
                )?);
                for pkg in &flake.packages {
                    flake_packages += &format!("${{{}.{}}}", name, pkg);
                }
            }
            flake_contents.replace("%FURTHER_FLAKE_PACKAGES%", &flake_packages)
        }
        None => flake_contents,
    };
    let input_list: Vec<&str> = inputs.iter().map(|i| &i.name[..]).collect();
    let input_list = input_list.join(", ");

    flake_contents = flake_contents
        .replace("#%INPUT_DEFS%", &format_input_defs(&inputs))
        .replace("#%INPUTS%", &input_list);

    flake_contents = nix_format(
        &flake_contents,
        &parsed_config.outside_nixpkgs.url,
        &lookup_github_tag(
            &parsed_config.outside_nixpkgs.url,
            &parsed_config.outside_nixpkgs.rev,
        )?,
    )?;

    //print!("{}", flake_contents);
    let mut git_path = flake_dir.to_path_buf();
    git_path.push(".git");
    if !git_path.exists() {
        let output = Command::new("git")
            .args(["init"])
            .current_dir(&flake_dir)
            .output()
            .context(format_f!("Failed create git repo in {flake_dir:?}"))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = format_f!(
                "Failed to init git repo in  {flake_dir:?}.\n Stdout {stdout:?}\nStderr: {stderr:?}"
            );
            return Err(anyhow!(msg));
        }
    }

    if use_generated_file_instead {
        if old_flake_contents != flake_contents {
            std::fs::write(flake_filename, flake_contents)?;
        }
        Ok(true)
    } else if old_flake_contents != flake_contents {
        std::fs::write(flake_filename, flake_contents)?;

        Ok(true)
    } else {
        println!("flake unchanged");
        Ok(false)
    }
}

fn format_input_defs(inputs: &Vec<InputFlake>) -> String {
    let mut out = "".to_string();
    for fl in inputs {
        let v_follows: Vec<String> = fl
            .follows
            .iter()
            .map(|x| format!("        inputs.{}.follows = \"{}\";", &x, &x))
            .collect();
        let str_follows = v_follows.join("\n");
        out.push_str(&format!(
            "
    {} = {{
        url = \"{}?rev={}\";
{}
    }};",
            fl.name, fl.url, fl.rev, &str_follows
        ))
    }
    out
}

fn extract_non_editable_python_packages(input: &[(String, String)]) -> Result<Vec<String>> {
    let mut res = Vec::new();
    for (name, version_constraint) in input.iter() {
        if version_constraint.starts_with("editable") {
            continue;
        }

        if version_constraint.contains("==")
            || version_constraint.contains('>')
            || version_constraint.contains('<')
            || version_constraint.contains('!')
        {
            res.push(format!("{}{}", name, version_constraint));
        } else if version_constraint.contains('=') {
            res.push(format!("{}={}", name, version_constraint));
        } else if version_constraint.is_empty() {
            res.push(name.to_string())
        } else {
            return Err(anyhow!(
                "invalid python version spec {}{}",
                name,
                version_constraint
            ));
        }
    }
    Ok(res)
}

fn pypi_deps_date_to_rev(date: NaiveDate) -> Result<String> {
    let query_date = date.and_hms(0, 0, 0);
    //chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00", date), "%Y-%m-%d %H:%M")
    //.context("Failed to parse pypi-deb-db date")?;
    let lowest =
        chrono::NaiveDateTime::parse_from_str("2020-04-22T08:54:49Z", "%Y-%m-%dT%H:%M:%SZ")
            .unwrap();
    if query_date < lowest {
        return Err(anyhow!(
            "Pypi-deps-db date too early. Starts at 2020-04-22T08:54:49Z"
        ));
    }
    let now: chrono::NaiveDateTime = chrono::Utc::now().naive_utc();
    if query_date > now {
        return Err(anyhow!("Pypi-deps-db date is in the future!"));
    }

    let store_path: PathBuf = ["flake", ".pypi-debs-db.lookup.json"].iter().collect();
    let query_date_str = query_date.format("%Y%m%d").to_string();
    fetch_cached(
        store_path,
        &query_date_str,
        PyPiDepsDBRetriever {
            query_date,
            query_date_str: query_date_str.to_string(),
        },
    )
}

struct PyPiDepsDBRetriever {
    query_date: NaiveDateTime,
    query_date_str: String,
}

impl PyPiDepsDBRetriever {
    fn pypi_deps_db_retrieve(page: i64) -> Result<HashMap<String, String>> {
        let url = format!(
            "http://api.github.com/repos/DavHau/pypi-deps-db/commits?per_page=100&page={}",
            page
        );
        let body: String = ureq::get(&url).call()?.into_string()?;
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse github commits api")?;
        let json = json
            .as_array()
            .context("No entries in github commits api?")?;
        let mut res = HashMap::new();
        for entry in json.iter() {
            let date = chrono::DateTime::parse_from_rfc3339(
                entry["commit"]["committer"]["date"]
                    .as_str()
                    .context("Empty committer date?")?,
            )?;
            let sha = entry["sha"].as_str().context("no sha on commit?")?;
            let str_date = date.format("%Y%m%d").to_string();
            //println!("{}, {}", &str_date, &sha);
            res.insert(str_date, sha.to_string());
        }
        Ok(res)
    }
}

impl Retriever for PyPiDepsDBRetriever {
    fn retrieve(&self) -> Result<HashMap<String, String>> {
        let now: chrono::NaiveDateTime = chrono::Utc::now().naive_utc();
        let mut page = now.signed_duration_since(self.query_date).num_days() / 35; //empirically..., just has to be close, not exact
        let mut known_mappings = HashMap::new();
        loop {
            let mut new_mappings = Self::pypi_deps_db_retrieve(page)?;
            if new_mappings.is_empty() {
                return Err(anyhow!(
                    "Could not find entry in pypi-deps-db (no more pages)"
                ));
            }
            let newest = newest_date(&new_mappings)?;
            let oldest = oldest_date(&new_mappings)?;
            for (k, v) in new_mappings.drain() {
                known_mappings.insert(k, v);
            }
            if known_mappings.contains_key(&self.query_date_str) {
                return Ok(known_mappings);
            } else {
                //it is not in there...
                if newest < self.query_date {
                    println!("{:?} too old", &self.query_date);
                    page -= 1;
                    if page == 0 {
                        return Err(anyhow!(
                            "Could not find entry in pypi-deps-db (arrived at latest entry)"
                        ));
                    }
                } else if oldest > self.query_date {
                    println!("{:?} too new", &self.query_date);
                    page += 1;
                }
            }
        }
    }
}

fn oldest_date(new_mappings: &HashMap<String, String>) -> Result<chrono::NaiveDateTime> {
    let oldest = new_mappings.keys().min().unwrap();
    Ok(chrono::NaiveDateTime::parse_from_str(
        &format!("{} 00:00", oldest),
        "%Y%m%d %H:%M",
    )?)
}
fn newest_date(new_mappings: &HashMap<String, String>) -> Result<chrono::NaiveDateTime> {
    let oldest = new_mappings.keys().max().unwrap();
    //println!("oldest {}", oldest);
    Ok(chrono::NaiveDateTime::parse_from_str(
        &format!("{} 00:00", oldest),
        "%Y%m%d %H:%M",
    )?)
}

fn lookup_github_tag(url: &str, tag_or_rev: &str) -> Result<String> {
    if tag_or_rev.len() == 40 || !url.starts_with("github:") {
        Ok(tag_or_rev.to_string())
    } else {
        let repo = url.strip_prefix("github:").unwrap();
        fetch_cached(
            [format!("flake/.github_{}.json", repo.replace("/", "_"))]
                .iter()
                .collect(),
            tag_or_rev,
            GitHubTagRetriever {
                repo: repo.to_string(),
            },
        )
        .with_context(|| format!("Looking up tag on {}", &url))
    }
}

trait Retriever {
    fn retrieve(&self) -> Result<HashMap<String, String>>;
}

fn fetch_cached(cache_filename: PathBuf, query: &str, retriever: impl Retriever) -> Result<String> {
    let mut known: HashMap<String, String> = match cache_filename.exists() {
        true => serde_json::from_str(&std::fs::read_to_string(&cache_filename)?)?,
        false => HashMap::new(),
    };
    if known.contains_key(query) {
        return Ok(known.get(query).unwrap().to_string());
    } else {
        let mut new = retriever.retrieve()?;
        for (k, v) in new.drain() {
            known.insert(k, v);
        }
        std::fs::write(cache_filename, serde_json::to_string_pretty(&json!(known))?)?;
        return Ok(known
            .get(query)
            .context(format!("Could not find query value: {}", query))?
            .to_string());
    }
}

struct GitHubTagRetriever {
    repo: String,
}

impl Retriever for GitHubTagRetriever {
    fn retrieve(&self) -> Result<HashMap<String, String>> {
        let mut res = HashMap::new();
        for page in 0..30 {
            let url = format!(
                "https://api.github.com/repos/{}/tags?per_page=100&page={}",
                &self.repo, page
            );
            let body: String = ureq::get(&url).call()?.into_string()?;
            let json: serde_json::Value =
                serde_json::from_str(&body).context("Failed to parse github tags api")?;
            let json = json.as_array().context("No entries in github tags api?")?;
            if json.is_empty() {
                break;
            }
            for entry in json {
                let name: String = entry["name"]
                    .as_str()
                    .context("No name found in github tags")?
                    .to_string();
                let sha: String = entry["commit"]["sha"]
                    .as_str()
                    .context("No sha found in github tags")?
                    .to_string();
                res.insert(name, sha);
            }
        }
        Ok(res)
    }
}

fn rebuild_flake(use_generated_file_instead: bool) -> Result<()> {
    let flake_dir: PathBuf = ["flake"].iter().collect();
    std::fs::write(
        flake_dir.join(".gitignore"),
        "result
run_scripts/
.*.json
",
    )?;
    println!("writing flake");
    let mut gitargs = vec!["add", "flake.nix", ".gitignore"];
    if flake_dir.join("flake.lock").exists() {
        gitargs.push("flake.nix");
    }
    let output = Command::new("git")
        .args(&gitargs)
        .current_dir(&flake_dir)
        .output()
        .context(format_f!("Failed git add flake.nix"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format_f!("Failed git add flake.nix. \n Stdout {stdout:?}\nStderr: {stderr:?}");
        return Err(anyhow!(msg));
    }

    if !use_generated_file_instead {
        Command::new("git")
            .args(["commit", "-m", "autocommit"])
            .current_dir(&flake_dir)
            .output()
            .context(format_f!("Failed git add flake.nix"))?;
    }
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg =
            format_f!("Failed git commit flake.nix. \n Stdout {stdout:?}\nStderr: {stderr:?}");
        return Err(anyhow!(msg));
    }
    let build_unfinished_file = flake_dir.join(".build_unfinished");
    std::fs::write(&build_unfinished_file, "in_progress")?;

    if Command::new("nix")
        .args(["build", "-v", "--show-trace"])
        .current_dir("flake")
        .status()?
        .success()
    {
        std::fs::remove_file(&build_unfinished_file)?;
        Ok(())
    } else {
        Err(anyhow!("flake building failed"))
    }
}

fn run_bash(script: &str) -> Result<()> {
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
            return Err(anyhow!("editable python package that was not present in file system (missing clone)? looking for package {} in {:?}", 
                               pkg, target_dir));
        }
        let egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
        if !egg_link.exists() {
            // so that changing python versions triggers a rebuild.
            to_build.push((safe_pkg, target_dir));
        }
    }
    if !to_build.is_empty() {
        for (safe_pkg, target_dir) in to_build.iter() {
            println!("Pip install {:?}", &target_dir);
            let td = tempdir::TempDir::new("anysnake_venv")?;
            let mut singularity_args: Vec<String> = vec![
                "exec".into(),
                "--userns".into(),
                "--no-home".into(),
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
            println!("Singularity cmd:\n\tsingularity \\");
            println!();
            let singularity_result = run_singularity(&singularity_args[..], nixpkgs_url)?;
            if !singularity_result.success() {
                return Err(anyhow!(
                    "Singularity pip install failed with exit code {}",
                    singularity_result.code().unwrap()
                ));
            }
            let target_egg_link = venv_dir.join(format!("{}.egg-link", safe_pkg));
            for dir_entry in walkdir::WalkDir::new(td.path()) {
                let dir_entry = dir_entry?;
                if let Some(filename) = dir_entry.file_name().to_str() {
                    if filename.ends_with(".egg-link") {
                        println!("found {:?} for {}", &safe_pkg, &filename);
                        std::fs::write(
                            target_egg_link,
                            std::fs::read_to_string(dir_entry.path())?,
                        )?;
                        break;
                    }
                };
            }
            //now clean up the empty directory we used to map the package
            //std::fs::remove_dir(venv_dir.join(safe_pkg))?;

            //st::fs::write(egg_link, format!("/venv/{}\n../
        }
    }
    //for (name, version_constraint) in input.iter() {
    //if version_constraint.starts_with("editable") {
    Ok(())
}
