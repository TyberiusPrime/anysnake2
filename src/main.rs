extern crate clap;
use anyhow::{anyhow, Context, Result};
use clap::{App, AppSettings, Arg};
use fstrings::{format_args_f, format_f, println_f};
use regex::Regex;
use serde_derive::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Deserialize, Debug)]
struct ConfigToml {
    nixpkgs: Nix,
    clone_regexps: Option<HashMap<String, String>>,
    clones: Option<HashMap<String, HashMap<String, String>>>,
    cmd: HashMap<String, Cmd>,
    rust: Option<Rust>,
    python: Option<Python>,
}

fn default_nixpkgs_url() -> String {
    "https://github.com/NixOS/nixpkgs".to_string()
}
#[derive(Deserialize, Debug)]
struct Nix {
    rev: String,
    #[serde(default = "default_nixpkgs_url")]
    url: String,
    packages: Option<Vec<String>>,
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
}
#[derive(Deserialize, Debug)]
struct Python {
    version: String,
    ecosystem_date: String,
    pypideps_rev: Option<String>,
    packages: HashMap<String, String>,
}

fn main() -> Result<()> {
    let date = "2021-01-04";
    println!("{} {:?}", date, pypydeb_date_to_commit(date));
    return Ok(());
    let matches = App::new("My Super Program")
        .version("1.0")
        .author("Kevin K. <kbknapp@gmail.com>")
        .about("Does awesome things")
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

    let config_file = matches.value_of("config_file").unwrap_or("anysnake2.toml");
    let raw_config = std::fs::read_to_string(config_file)?;
    let mut parsed_config: ConfigToml =
        toml::from_str(&raw_config).context(format_f!("Failure parsing {config_file}"))?;

    let cmd = match matches.subcommand() {
        (name, Some(_subcommand)) => name,
        _ => "default",
    };
    if !parsed_config.cmd.contains_key(cmd) {
        return Err(anyhow!(
            "Cmd {} not found. Available: {:?}",
            cmd,
            parsed_config.cmd.keys()
        ));
    }

    lookup_clones(&mut parsed_config)?;
    println_f!("Hello, world! ");

    perform_clones(&parsed_config)?;
    let flake_changed = write_flake(&parsed_config)?;
    println_f!("Done");

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
                            .args(["clone", furl])
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
                std::fs::write(clone_log, serde_json::to_string_pretty(&json!(known_clones))?)?;
            }
        }
        None => {}
    };

    Ok(())
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
        .replace("%NIXPKG_URL%", &parsed_config.nixpkgs.url)
        .replace("%NIXPKG_REV%", &parsed_config.nixpkgs.rev);
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
    flake_contents = match &parsed_config.rust {
        Some(rust) => flake_contents.replace(
            "%RUST%",
            &format!("${{rust-bin.stable.\"{}\".default}}", &rust.version),
        ),
        None => flake_contents,
    };
    flake_contents = match &parsed_config.python {
        Some(python) => {
            if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                return Err(anyhow!(
                        format!("Python version must be x.y (not x.y.z ,z is given by nixpkgs version). Was '{}'", &python.version)));
            }
            let python_major_minor = format!("python{}", python.version.replace(".", ""));

            let python_packages = extract_non_editable_python_packages(&python.packages)?;
            let python_packages = python_packages.join("\n");

            flake_contents
                .replace("%PYTHON_MAJOR_MINOR%", &python_major_minor)
                .replace("%PYTHON_PACKAGES%", &python_packages)
        }
        None => flake_contents,
    };

    print!("{}", flake_contents);
    let res = if old_flake_contents != flake_contents {
        std::fs::write(flake_filename, template)?;
        Ok(true)
    } else {
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

fn pypydeb_date_to_commit(date: &str) -> Result<String> {
    let query_date =
        chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00", date), "%Y-%m-%d %H:%M")
            .context("Failed to parse pypi-deb-db date")?;
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
    let mut known_mappings: HashMap<String, String> = match store_path.exists() {
        true => serde_json::from_str(&std::fs::read_to_string(&store_path)?)?,
        false => HashMap::new(),
    };
    let query_date_str = query_date.format("%Y%m%d").to_string();
    if known_mappings.contains_key(&query_date_str) {
        return Ok(known_mappings.get(&query_date_str).unwrap().to_string());
    } else {
        let mut page = now.signed_duration_since(query_date).num_days() / 35; //empirically...
        while true {
            let new_mappings = pypi_deps_db_retrieve(page)?;
            if new_mappings.len() == 0 {
                return Err(anyhow!(
                    "Could not find entry in pypi-deps-db (no more pages)"
                ));
            }
            for (k, v) in new_mappings.iter() {
                known_mappings.insert(k.to_string(), v.to_string());
            }
            if known_mappings.contains_key(&query_date_str) {
                std::fs::write(store_path, serde_json::to_string_pretty(&json!(known_mappings))?)?;
                return Ok(known_mappings.get(&query_date_str).unwrap().to_string());
            } else {
                //it is not in there...
                if newest_date(&new_mappings)? < query_date {
                    println!("{:?} too old", query_date);
                    page -= 1;
                    if page == 0 {
                        return Err(anyhow!(
                            "Could not find entry in pypi-deps-db (arrived at latest entry)"
                        ));
                    }
                } else if oldest_date(&new_mappings)? > query_date {
                    println!("{:?} too new", query_date);
                    page += 1;
                }
            }
        }
    }
    Err(anyhow!("Not found"))
}

#[derive(Deserialize, Debug)]
struct CommitEntry {
    sha: String,
    commit: Commit,
}
#[derive(Deserialize, Debug)]
struct Commit {
    committer: Commiter,
}
#[derive(Deserialize, Debug)]
struct Commiter {
    date: String,
}

fn pypi_deps_db_retrieve(page: i64) -> Result<HashMap<String, String>> {
    let url = format!(
        "http://api.github.com/repos/DavHau/pypi-deps-db/commits?per_page=100&page={}",
        page
    );
    println!("{}", url);
    let body: String = ureq::get(&url)
        //.set("Example-Header", "header value")
        .call()?
        .into_string()?;
    let json = serde_json::from_str(&body);
    let json: Vec<CommitEntry> = match json {
        Ok(x) => Ok(x),
        Err(e) => {
            println!("{}", body);
            Err(e)
        }
    }?;

    let mut res = HashMap::new();
    for entry in json.iter() {
        let date = chrono::DateTime::parse_from_rfc3339(&entry.commit.committer.date)?;
        let str_date = date.format("%Y%m%d").to_string();
        println!("{}, {}", &str_date, &entry.sha);
        res.insert(str_date, entry.sha.clone());
    }
    Ok(res)
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
