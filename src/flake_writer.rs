use crate::config;
use anyhow::{bail, Context, Result, anyhow};
use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use log::{debug, trace};

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

pub fn write_flake(
    flake_dir: &Path,
    parsed_config: &config::ConfigToml,
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
                bail!(
                        format!("Python version must be x.y (not x.y.z ,z is given by nixpkgs version). Was '{}'", &python.version));
            }
            let python_major_minor = format!("python{}", python.version.replace(".", ""));

            let mut out_python_packages = extract_non_editable_python_packages(python_packages)?;
            out_python_packages.sort();
            let out_python_packages = out_python_packages.join("\n");

            let ecosystem_date = python
                .parsed_ecosystem_date()
                .context("Failed to parse python.ecosystem-date")?;
            let pypi_debs_db_rev = pypi_deps_date_to_rev(ecosystem_date)?;

            inputs.push(InputFlake::new(
                "mach-nix",
                &parsed_config.mach_nix.url,
                &parsed_config.mach_nix.rev,
                &["nixpkgs", "flake-utils", "pypi-deps-db"],
            )?);

            inputs.push(InputFlake::new(
                "pypi-deps-db",
                "github:DavHau/pypi-deps-db",
                &pypi_debs_db_rev,
                &["nixpkgs", "mach-nix"],
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
            .args(&["init"])
            .current_dir(&flake_dir)
            .output()
            .context(format!("Failed create git repo in {:?}", flake_dir))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = format!(
                "Failed to init git repo in  {:?}.\n Stdout {:?}\nStderr: {:?}",
                flake_dir, stdout, stderr
            );
            bail!(msg);
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
        debug!("flake unchanged");
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
            res.push(format!("{}=={}", name, version_constraint));
            //bail!("invalid python version spec {}{}", name, version_constraint);
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
        bail!("Pypi-deps-db date too early. Starts at 2020-04-22T08:54:49Z");
    }
    let now: chrono::NaiveDateTime = chrono::Utc::now().naive_utc();
    if query_date > now {
        bail!("Pypi-deps-db date is in the future!");
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
                bail!("Could not find entry in pypi-deps-db (no more pages)");
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
                    trace!("{:?} too old", &self.query_date);
                    page -= 1;
                    if page == 0 {
                        bail!("Could not find entry in pypi-deps-db (arrived at latest entry)");
                    }
                } else if oldest > self.query_date {
                    trace!("{:?} too new", &self.query_date);
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

pub fn lookup_github_tag(url: &str, tag_or_rev: &str) -> Result<String> {
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
