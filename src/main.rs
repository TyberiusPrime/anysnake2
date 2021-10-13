extern crate clap;
use anyhow::{anyhow, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use clap::{App, AppSettings, Arg};
use const_format::concatcp;
use fstrings::{format_args_f, format_f, println_f};
use regex::Regex;
use serde_derive::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::io::{self, BufRead};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/* TODO
 *
 * running container managment
 * no flake-write-mode
 * arbitrary flake inclusion
 * python requirements parsing 
 * run_scripts in flakes/run_scripts
 * Python venv
 * R
 * Bootstrapping - using a defined anysnake2 version
 *  ( easy: if a parameter is passed and it matches the toml, or if anysnake2.version = 'dev' do
 *  nothing. otheriwise nix flake run...
 *
*/

const DEFAULT_MACH_NIX_REPO: &str = "DavHau/mach-nix";
const DEFAULT_MACH_NIX_URL: &str = concatcp!("github:", DEFAULT_MACH_NIX_REPO);
const DEFAULT_MACH_NIX_REV: &str = "3.3.0";

const DEFAULT_RUST_OVERLAY_REPO: &str = "oxalica/rust-overlay";
const DEFAULT_RUST_OVERLAY_URL: &str = concatcp!("github:", DEFAULT_RUST_OVERLAY_REPO);
const DEFAULT_RUST_OVERLAY_REV: &str = "08de2ff90cc08e7f9523ad97e4c1653b09f703ec";

const DEFAULT_NIXPKGS_REPO: &str = "NixOS/nixpkgs";
const DEFAULT_NIXPKGS_URL: &str = concatcp!("github:", DEFAULT_NIXPKGS_REPO);
const DEFAULT_FLAKE_UTIL_REV: &str = "7e5bf3925f6fbdfaf50a2a7ca0be2879c4261d19";

#[derive(Deserialize, Debug)]
struct ConfigToml {
    home: Option<String>,
    nixpkgs: Nix,
    flake_util: Option<FlakeUtil>,
    clone_regexps: Option<HashMap<String, String>>,
    clones: Option<HashMap<String, HashMap<String, String>>>,
    cmd: HashMap<String, Cmd>,
    rust: Option<Rust>,
    python: Option<Python>,
    volumes_ro: Option<HashMap<String, String>>,
    volumes_rw: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Debug)]
struct Nix {
    rev: String,
    url: Option<String>,
    packages: Option<Vec<String>>,
}
#[derive(Deserialize, Debug)]
struct FlakeUtil {
    rev: String,
}

#[derive(Deserialize, Debug)]
struct Cmd {
    run: String,
    pre_run_outside: Option<String>,
    post_run_inside: Option<String>,
    post_run_outside: Option<String>,
    port_range: Option<(u16, u16)>,
}
#[derive(Deserialize, Debug)]
struct Rust {
    version: String,
    rust_overlay_rev: Option<String>,
    rust_overlay_url: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Python {
    version: String,
    //#[serde(with = "my_date_format")]
    //ecosystem_date: DateTime<Utc>,
    ecosystem_date: String,
    pypideps_rev: Option<String>,
    packages: HashMap<String, String>,
    mach_nix_rev: Option<String>,
    mach_nix_url: Option<String>,
}

fn parse_my_date(s: &str) -> Result<chrono::NaiveDate> {
    const FORMAT: &'static str = "%Y-%m-%d %H:%M:%S";
    use chrono::TimeZone;
    Ok(chrono::Utc
        .datetime_from_str(&format!("{} 00:00:00", s), FORMAT)?
        .naive_utc()
        .date())
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
            Arg::with_name("cwd")
                .long("cwd")
                .value_name("DIRECTORY")
                .help("change working directory before executing")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        )
        /* .subcommand(SubCommand::with_name("test")
        .about("controls testing features")
        .version("1.3")
        .author("Someone E. <someone_else@other.com>")
        .arg(Arg::with_name("debug")
            .short("d")
            .help("print debug information verbosely")))
            */
        .get_matches();

    match matches.value_of("cwd") {
        Some(cwd) => {
            println!("changing working directory to {}", &cwd);
            std::env::set_current_dir(cwd)?
        }
        None => {}
    }

    let config_file = matches.value_of("config_file").unwrap_or("anysnake2.toml");
    let raw_config = std::fs::read_to_string(config_file)?;
    let mut parsed_config: ConfigToml =
        toml::from_str(&raw_config).context(format_f!("Failure parsing {config_file}"))?;

    let cmd = match matches.subcommand() {
        (name, Some(_subcommand)) => name,
        _ => "default",
    };
    if !parsed_config.cmd.contains_key(cmd) && cmd != "build" {
        return Err(anyhow!(
            "Cmd {} not found. Available: {:?}",
            cmd,
            parsed_config.cmd.keys()
        ));
    }

    lookup_clones(&mut parsed_config)?;
    perform_clones(&parsed_config)?;
    let python_requirements_from_clones = find_python_requirements_for_clones(&parsed_config)?;

    let flake_changed = write_flake(&parsed_config)?;
    let build_output: PathBuf = ["flake", "result"].iter().collect();
    if flake_changed || !build_output.exists() {
        println!("{}", "Rebuilding");
        rebuild_flake()?;
    }

    let home_dir: PathBuf = {
        [replace_env_vars(
            parsed_config.home.as_deref().unwrap_or("$HOME"),
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
        let run_script = run_template.replace("%RUN%", &cmd_info.run);
        let post_run_script =
            run_template.replace("%RUN%", cmd_info.post_run_inside.as_deref().unwrap_or(""));

        let outer_run_sh: PathBuf = [".outer_run.sh"].iter().collect(); //todo: tempfile
        let outer_run_sh_str: String = outer_run_sh
            .clone()
            .into_os_string()
            .to_string_lossy()
            .to_string();
        let run_sh: PathBuf = [".run.sh"].iter().collect(); //todo: tempfile
        let run_sh_str: String = run_sh
            .clone()
            .into_os_string()
            .to_string_lossy()
            .to_string();
        let post_run_sh: PathBuf = [".post_run.sh"].iter().collect(); //todo: tempfile
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
            (home_dir_str).into(),
        ];
        let mut binds = Vec::new();
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

        match parsed_config.volumes_ro {
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
        match parsed_config.volumes_rw {
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
            singularity_args.push(format!("{}:{}:{}", from, to, opts).into());
        }

        singularity_args.push("flake/result/rootfs".into());
        singularity_args.push("/bin/bash".into());
        singularity_args.push("/anysnake2/outer_run.sh".into());
        println!("{}", "Singularity cmd:\n\tsingularity \\");
        for s in &singularity_args {
            println!("\t\t{} \\", s);
        }
        println!("{}", "");
        let singularity_result = Command::new("singularity")
            .args(singularity_args)
            .status()?;

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

fn find_python_requirements_for_clones(parsed_config: &ConfigToml) -> Result<Vec<String>> {
    let mut res = Vec::new();
    match &parsed_config.clones {
        Some(clones) => {
            for (target_dir, name_urls) in clones.iter() {
                for (name, url) in name_urls.iter() {
                    let requirement_file: PathBuf =
                        [target_dir, name, "requirements.txt"].iter().collect();
                    if requirement_file.exists() {
                        for line in read_lines(requirement_file)?
                            .map(|line| line.unwrap_or_else(|_| "".to_string()))
                            .map(|line| line.trim().to_string())
                            .filter(|line| !line.is_empty() && !line.starts_with("#"))
                        {
                            res.push(line);
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
                                for k in reqs.drain(..) {
                                    res.push(k);
                                }
                            }
                        };
                    }
                }
            }
        }
        None => {}
    }
    Ok(res)
}

fn parse_python_config_file(setup_cfg_file: &Path) -> Result<Vec<String>> {
    println!("Parsing {:?}", &setup_cfg_file);
    let mut setup_cfg = configparser::ini::Ini::new();
    setup_cfg
        .load(setup_cfg_file)
        .map_err(|e| anyhow!("{}", e))?;
    let install_requires = setup_cfg.get("options", "install_requires").ok_or(anyhow!("No install_requires section"))?;
    println!("install reqs: {}", install_requires);
    Err(anyhow!("Could not parse"))
}

fn write_flake(parsed_config: &ConfigToml) -> Result<bool> {
    let template = std::include_str!("flake_template.nix");
    let flake_dir: PathBuf = ["flake"].iter().collect();
    std::fs::create_dir_all(&flake_dir)?;
    let flake_filename: PathBuf = ["flake", "flake.nix"].iter().collect();
    let old_flake_contents = {
        if flake_filename.exists() {
            std::fs::read_to_string(&flake_filename)?
        } else {
            "".to_string()
        }
    };
    let mut flake_contents = template
        .replace(
            "%NIXPKG_URL%",
            &parsed_config
                .nixpkgs
                .url
                .as_deref()
                .unwrap_or(DEFAULT_NIXPKGS_URL),
        )
        .replace(
            "%NIXPKG_REV%",
            &(if &parsed_config
                .nixpkgs
                .url
                .as_deref()
                .unwrap_or(DEFAULT_NIXPKGS_URL)
                == &DEFAULT_NIXPKGS_URL
            {
                // if you change the url, you are on your own for the tags. Sorry.
                lookup_nix_rev(&parsed_config.nixpkgs.rev)?
            } else {
                parsed_config.nixpkgs.rev.to_string()
            }),
        );
    flake_contents = match &parsed_config.nixpkgs.packages {
        Some(pkgs) => {
            let pkgs: String = pkgs
                .iter()
                .map(|x| format!("${{{}}}\n", x))
                .collect::<Vec<String>>()
                .join("\n");
            flake_contents.replace("%NIXPKGSPKGS%", &pkgs)
        }
        None => flake_contents,
    };
    flake_contents = flake_contents.replace(
        "%FLAKE_UTIL_REV%",
        match &parsed_config.flake_util {
            Some(fu) => &fu.rev,
            None => &DEFAULT_FLAKE_UTIL_REV,
        },
    );
    let (mut flake_contents, rust_overlay_rev, rust_overlay_url) = match &parsed_config.rust {
        Some(rust) => (
            flake_contents.replace(
                "%RUST%",
                // skipping the rust-docs saves about 270 mb ¯\_(ツ)_/¯
                &format!("pkgs.rust-bin.stable.\"{}\".minimal.override {{ extensions = [ \"rustfmt\" \"clippy\"]; }}", &rust.version),
            ),
            rust.rust_overlay_rev
                .as_deref()
                .unwrap_or(DEFAULT_RUST_OVERLAY_REV),
            rust.rust_overlay_url
                .as_deref()
                .unwrap_or(DEFAULT_RUST_OVERLAY_URL),
        ),
        None => (
                 flake_contents.replace(
                "%RUST%",
                "null",
            ),

            DEFAULT_RUST_OVERLAY_REV,
            DEFAULT_RUST_OVERLAY_URL,
        ),
    };
    flake_contents = flake_contents
        .replace("%RUST_OVERLAY_REV%", rust_overlay_rev)
        .replace("%RUST_OVERLAY_URL%", rust_overlay_url);
    flake_contents = match &parsed_config.python {
        Some(python) => {
            if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                return Err(anyhow!(
                        format!("Python version must be x.y (not x.y.z ,z is given by nixpkgs version). Was '{}'", &python.version)));
            }
            let python_major_minor = format!("python{}", python.version.replace(".", ""));

            let python_packages = extract_non_editable_python_packages(&python.packages)?;
            let python_packages = python_packages.join("\n");

            let ecosystem_date = parse_my_date(&python.ecosystem_date)
                .context("Failed to parse python.ecosystem-date")?;
            let pypi_debs_db_rev = pypi_deps_date_to_rev(ecosystem_date)?;
            let mach_nix_rev = python
                .mach_nix_rev
                .as_deref()
                .unwrap_or(DEFAULT_MACH_NIX_REV);
            let mach_nix_url = python
                .mach_nix_url
                .as_deref()
                .unwrap_or(DEFAULT_MACH_NIX_URL);

            let mach_nix_rev = if mach_nix_url == DEFAULT_MACH_NIX_URL {
                lookup_mach_nix_rev(mach_nix_rev)? //todo: turn into cow
            } else {
                mach_nix_rev.to_string()
            };

            flake_contents
                .replace("%PYTHON_MAJOR_MINOR%", &python_major_minor)
                .replace("%PYTHON_PACKAGES%", &python_packages)
                .replace("%PYPI_DEPS_DB_REV%", &pypi_debs_db_rev)
                .replace("%MACH_NIX_REV%", &mach_nix_rev)
                .replace("%MACH_NIX_URL%", &mach_nix_url)
        }
        None => flake_contents,
    };

    //print!("{}", flake_contents);
    let res = if old_flake_contents != flake_contents {
        std::fs::write(flake_filename, flake_contents)?;
        println!("writing flake");
        Ok(true)
    } else {
        println!("flake unchanged");
        Ok(false)
    };
    let mut git_path = flake_dir.clone();
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
                "Failed to init git repo in  {flake_dir:?}.\
                                                \n Stdout {stdout:?}\nStderr: {stderr:?}"
            );
            return Err(anyhow!(msg));
        }
        let output = Command::new("git")
            .args(["add", "flake.nix"])
            .current_dir(&flake_dir)
            .output()
            .context(format_f!("Failed git add flake.nix"))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg =
                format_f!("Failed git add flake.nix. \n Stdout {stdout:?}\nStderr: {stderr:?}");
            return Err(anyhow!(msg));
        }
    }

    res
}

fn extract_non_editable_python_packages(input: &HashMap<String, String>) -> Result<Vec<String>> {
    let mut res = Vec::new();
    for (name, version_constraint) in input.iter() {
        if version_constraint.starts_with("editable") {
            continue;
        }
        if version_constraint.contains(">") {
            res.push(format!("{}{}", name, version_constraint));
        } else {
            res.push(format!("{}=={}", name, version_constraint));
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
        Err(anyhow!(
            "Pypi-deps-db date too early. Starts at 2020-04-22T08:54:49Z"
        ))?
    }
    let now: chrono::NaiveDateTime = chrono::Utc::now().naive_utc();
    if query_date > now {
        Err(anyhow!("Pypi-deps-db date is in the future!"))?
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
            println!("{}, {}", &str_date, &sha);
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
            if new_mappings.len() == 0 {
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
    println!("oldest {}", oldest);
    Ok(chrono::NaiveDateTime::parse_from_str(
        &format!("{} 00:00", oldest),
        "%Y%m%d %H:%M",
    )?)
}

fn lookup_github_tag(repo: &str, tag_or_rev: &str) -> Result<String> {
    if tag_or_rev.len() == 40 {
        Ok(tag_or_rev.to_string())
    } else {
        fetch_cached(
            [format!("flake/.github_{}.json", repo.replace("/", "_"))]
                .iter()
                .collect(),
            tag_or_rev,
            GitHubTagRetriever {
                repo: repo.to_string(),
            },
        )
    }
}

fn lookup_nix_rev(tag_or_rev: &str) -> Result<String> {
    lookup_github_tag(DEFAULT_NIXPKGS_REPO, tag_or_rev).context("Failed to lookup nixpkgs tag")
}

fn lookup_mach_nix_rev(tag_or_rev: &str) -> Result<String> {
    lookup_github_tag(DEFAULT_MACH_NIX_REPO, tag_or_rev).context("Failed to lookup mach-nix tag")
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

fn rebuild_flake() -> Result<()> {
    if Command::new("nix")
        .args(["build", "-v", "--show-trace"])
        .current_dir("flake")
        .status()?
        .success()
    {
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
    drop(child_stdin); // close
    let ecode = child.wait().context("Failed to wait on bash")?;
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
