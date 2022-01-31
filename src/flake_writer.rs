use crate::config;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use ex::fs;
use itertools::Itertools;
use log::{debug, trace};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::run_without_ctrl_c;

struct InputFlake {
    name: String,
    url: String,
    rev: String,
    follows: Vec<String>,
}

impl InputFlake {
    fn new(
        name: &str,
        url: &str,
        rev: &str,
        follows: &[&str],
        flake_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        let url = if url.ends_with('/') {
            url.strip_suffix('/').unwrap()
        } else {
            url
        };
        Ok(InputFlake {
            name: name.to_string(),
            url: url.to_string(),
            rev: lookup_github_tag(url, rev, flake_dir)?,
            follows: follows.iter().map(|x| x.to_string()).collect(),
        })
    }
}

#[allow(clippy::vec_init_then_push)]
pub fn write_flake(
    flake_dir: impl AsRef<Path>,
    parsed_config: &mut config::ConfigToml,
    python_packages: &[(String, String)],
    python_build_packages: &[(String, HashMap<String, String>)],
    use_generated_file_instead: bool,
) -> Result<bool> {
    let template = std::include_str!("flake_template.nix");
    let flake_filename: PathBuf = if use_generated_file_instead {
        flake_dir.as_ref().join("flake.generated.nix")
    } else {
        flake_dir.as_ref().join("flake.nix")
    };
    let old_flake_contents = {
        if flake_filename.exists() {
            fs::read_to_string(&flake_filename)?
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
        &flake_dir,
    )?);
    let mut nixpkgs_pkgs = match &parsed_config.nixpkgs.packages {
        Some(pkgs) => pkgs.clone(),
        None => Vec::new(),
    };
    if let Some(_rust_ver) = &parsed_config.rust.version {
        nixpkgs_pkgs.push("stdenv.cc".to_string()); // needed to actually build something with rust
    }

    nixpkgs_pkgs.push("cacert".to_string()); //so we have SSL certs inside

    inputs.push(InputFlake::new(
        "flake-utils",
        &parsed_config.flake_util.url,
        &parsed_config.flake_util.rev,
        &["nixpkgs"],
        &flake_dir,
    )?);

    let mut overlays = Vec::new();
    let mut rust_extensions = vec!["rustfmt", "clippy"];

    flake_contents = match &parsed_config.python {
        Some(python) => {
            if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                bail!(
                        format!("Python version must be x.y (not x.y.z ,z is given by nixpkgs version). Was '{}'", &python.version));
            }
            let python_major_dot_minor = &python.version;
            let python_major_minor = format!("python{}", python.version.replace(".", ""));

            let mut out_python_packages = extract_non_editable_python_packages(python_packages)?;
            if parsed_config.r.is_some() {
                out_python_packages.push("rpy2".to_string());
            }
            out_python_packages.sort();
            let out_python_packages = out_python_packages.join("\n");

            let out_python_build_packages = format_python_build_packages(python_build_packages)?;

            let ecosystem_date = python
                .parsed_ecosystem_date()
                .context("Failed to parse python.ecosystem-date")?;
            let pypi_debs_db_rev = pypi_deps_date_to_rev(ecosystem_date, &flake_dir)?;

            inputs.push(InputFlake::new(
                "mach-nix",
                &parsed_config.mach_nix.url,
                &parsed_config.mach_nix.rev,
                &["nixpkgs", "flake-utils", "pypi-deps-db"],
                &flake_dir,
            )?);

            inputs.push(InputFlake::new(
                "pypi-deps-db",
                "github:DavHau/pypi-deps-db",
                &pypi_debs_db_rev,
                &["nixpkgs", "mach-nix"],
                &flake_dir,
            )?);

            flake_contents
                //.replace("%PYTHON_MAJOR_MINOR%", &python_major_minor)
                .replace("%PYTHON_PACKAGES%", &out_python_packages)
                .replace("%PYTHON_BUILD_PACKAGES%", &out_python_build_packages)
                .replace("%PYTHON_MAJOR_DOT_MINOR%", &python_major_dot_minor)
                .replace("%PYPI_DEPS_DB_REV%", &pypi_debs_db_rev)
                .replace(
                    "\"%MACHNIX%\"",
                    &format!(
                        "
    (import mach-nix) {{
          inherit pkgs;
          pypiDataRev = pypi-deps-db.rev;
          pypiDataSha256 = pypi-deps-db.narHash;
          python = \"{python_major_minor}\";
        }}
        ",
                        python_major_minor = &python_major_minor
                    ),
                )
        }
        None => flake_contents
            .replace("\"%MACHNIX%\"", "null")
            .replace("%DEVELOP_PYTHON_PATH%", "")
            .replace("%PYTHON_BUILD_PACKAGES%", ""),
    };

    flake_contents = match &parsed_config.flakes {
        Some(flakes) => {
            let mut flake_packages = "".to_string();
            let mut names: Vec<&String> = flakes.keys().collect();
            names.sort();
            for name in names {
                let flake = flakes.get(name).unwrap();
                let rev_follows: Vec<&str> = match &flake.follows {
                    Some(f) => f.iter().map(|x| &x[..]).collect(),
                    None => Vec::new(),
                };
                if flake.url.starts_with("path:/") {
                    return Err(anyhow!("flake urls must not start with path:/. These handle ?rev= wrong. Use just an absolute path instead"));
                }
                inputs.push(InputFlake::new(
                    name,
                    &flake
                        .url
                        .replace("$ANYSNAKE_ROOT", &parsed_config.get_root_path_str()?),
                    &flake.rev,
                    &rev_follows[..],
                    &flake_dir,
                )?);
                for pkg in &flake.packages {
                    flake_packages += &format!("${{{}.{}}}", name, pkg);
                }
            }
            flake_contents.replace("%FURTHER_FLAKE_PACKAGES%", &flake_packages)
        }
        None => flake_contents.replace("%FURTHER_FLAKE_PACKAGES%", ""),
    };
    let dev_shell_inputs = match &parsed_config.dev_shell.inputs {
        Some(dvi) => dvi.join(" "),
        None => "".to_string(),
    };
    flake_contents = flake_contents.replace("#%DEVSHELL_INPUTS%", &dev_shell_inputs);

    flake_contents = match &parsed_config.r {
        Some(r_config) => {
            inputs.push(InputFlake::new(
                "r_ecosystem_track",
                &r_config.r_ecosystem_track_url,
                &r_config.ecosystem_tag,
                &["flake-utils"],
                &flake_dir,
            )?);

            let r_packages = format!(
                "
                R_tracked = if (builtins.hasAttr \"R\" r_ecosystem_track) then
                      r_ecosystem_track.R.${{system}}
                    else
                      (builtins.elemAt r_ecosystem_track.rWrapper.${{system}}.buildInputs 0); # fall back for older r_ecosystem_tracks w/o R export
                r_packages = with r_ecosystem_track.rPackages.${{system}}; [ {} ];
                rWrapper = r_ecosystem_track.rWrapper.${{system}}.override{{ packages = r_packages; }};
                ",
                r_config.packages.join(" ")
            );
            overlays.push(
                "(final: prev: { 
                R = R_tracked; 
                rPackages = r_ecosystem_track.rPackages.${system};
                }) "
                .to_string(),
            );

            nixpkgs_pkgs.push("rWrapper".to_string());
            flake_contents.replace("#%RPACKAGES%", &r_packages)
        }
        None => flake_contents.replace("#%RPACKAGES%", "rWrapper = null;"),
    };

    let mut jupyter_kernels = String::new();
    let jupyter_included = python_packages.iter().any(|(k, _)| k == "jupyter");
    if let Some(r) = &parsed_config.r {
        // install R kernel
        if r.packages.contains(&"IRkernel".to_string()) && jupyter_included {
            jupyter_kernels.push_str(
                "
            mkdir $out/rootfs/usr/share/jupyter/kernels/R
            cp $out/rootfs/R_libs/IRkernel/kernelspec/* $out/rootfs/usr/share/jupyter/kernels/R -r
            ",
            );
        }
    }
    if nixpkgs_pkgs.contains(&"evcxr".to_string()) {
        jupyter_kernels.push_str(
            "
            JUPYTER_PATH=$out/rootfs/usr/share/jupyter $out/rootfs/bin/evcxr_jupyter --install
        ",
        );
        rust_extensions.push("rust-src");
    }
    if !jupyter_kernels.is_empty() && jupyter_included {
        jupyter_kernels = "
            mv $out/rootfs/usr/share/jupyter/kernels $out/rootfs/usr/share/jupyter/kernels_
            mkdir $out/rootfs/usr/share/jupyter/kernels
            cp $out/rootfs/usr/share/jupyter/kernels_/* $out/rootfs/usr/share/jupyter/kernels -r
            unlink $out/rootfs/usr/share/jupyter/kernels_
            "
        .to_string()
            + &jupyter_kernels;
    }

    flake_contents = flake_contents.replace("#%INSTALL_JUPYTER_KERNELS%", &jupyter_kernels);

    let nixpkgs_pkgs: String = nixpkgs_pkgs
        .iter()
        .map(|x| format!("${{{}}}\n", x))
        .collect::<Vec<String>>()
        .join("");

    flake_contents = match &parsed_config.rust.version {
        Some(version) => {
            inputs.push(InputFlake::new(
                "rust-overlay",
                &parsed_config.rust.rust_overlay_url,
                &parsed_config.rust.rust_overlay_rev,
                &["nixpkgs", "flake-utils"],
                &flake_dir,
            )?);
            overlays.push("(import rust-overlay)".to_string());
            let str_rust_extensions: Vec<String> = rust_extensions
                .into_iter()
                .map(|x| format!("\"{}\"", x))
                .collect();
            let str_rust_extensions: String = str_rust_extensions.join(" ");

            flake_contents.replace("\"%RUST%\"", &format!("pkgs.rust-bin.stable.\"{}\".minimal.override {{ extensions = [ {rust_extensions}]; }}", version, rust_extensions = str_rust_extensions))
        }
        None => flake_contents.replace("\"%RUST%\"", "\"\""),
    };

    flake_contents = flake_contents.replace("%NIXPKGS_PACKAGES%", &nixpkgs_pkgs);
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
            &flake_dir,
        )?,
        &flake_dir,
    )?;

    if !overlays.is_empty() {
        flake_contents = flake_contents.replace(
            "\"%OVERLAY_AND_PACKAGES%\"",
            &("[".to_string() + &overlays.join(" ") + "]"),
        );
    } else {
        flake_contents = flake_contents.replace("\"%OVERLAY_AND_PACKAGES%\"", "[]");
    }

    //print!("{}", flake_contents);
    let mut git_path = flake_dir.as_ref().to_path_buf();
    git_path.push(".git");
    if !git_path.exists() {
        let output = Command::new("git")
            .args(&["init"])
            .current_dir(&flake_dir)
            .output()
            .context(format!(
                "Failed create git repo in {:?}",
                flake_dir.as_ref()
            ))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = format!(
                "Failed to init git repo in  {:?}.\n Stdout {:?}\nStderr: {:?}",
                flake_dir.as_ref(),
                stdout,
                stderr
            );
            bail!(msg);
        }
    }

    let mut gitargs = vec!["add", "flake.nix", ".gitignore"];
    if flake_dir.as_ref().join("flake.lock").exists() {
        gitargs.push("flake.lock");
    }

    let res = if use_generated_file_instead {
        if old_flake_contents != flake_contents {
            fs::write(flake_filename, flake_contents)?;
        }
        Ok(true)
    } else if old_flake_contents != flake_contents {
        fs::write(&flake_filename, flake_contents)
            .with_context(|| format!("failed writing {:?}", &flake_filename))?;

        Ok(true)
    } else {
        debug!("flake unchanged");
        Ok(false)
    };
    fs::write(
        flake_dir.as_ref().join(".gitignore"),
        "result
run_scripts/
.*.json
.gc_roots
",
    )?;

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

    res
}

fn format_input_defs(inputs: &[InputFlake]) -> String {
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
        url = \"{}{}rev={}\";
{}
    }};",
            fl.name,
            fl.url,
            if !fl.url.contains("?") { "?" } else { "&" },
            fl.rev,
            &str_follows
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

fn src_to_nix(src: &HashMap<String, String>) -> String {
    let mut res = Vec::new();
    for (k, v) in src.iter().sorted_by_key(|x| x.0) {
        if k != "method" {
            res.push(format!("\"{}\" = \"{}\";", k, v));
        }
    }
    res.join("\n")
}

fn format_python_build_packages(input: &[(String, HashMap<String, String>)]) -> Result<String> {
    let mut res: String = "packagesExtra = [".into();
    for (key, spec) in input.iter().sorted_by_key(|x| &x.0) {
        res.push_str(&format!(
            "
              (mach-nix_.buildPythonPackage {{
                src = pkgs.{} {{ # {}
                    {}
                }};
                requirementsExtra = python_requirements;
              }})",
            spec.get("method")
                .expect("Missing 'method' on python build package definition"),
            key,
            src_to_nix(spec)
        ));
    }
    res.push_str("];");
    Ok(res)
}

fn pypi_deps_date_to_rev(date: NaiveDate, flake_dir: impl AsRef<Path>) -> Result<String> {
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

    let store_path: PathBuf = flake_dir
        .as_ref()
        .join(".pypi-debs-db.lookup.json")
        .iter()
        .collect();
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

fn get_basic_auth_header(user: &str, pass: &str) -> String {
    let usrpw = String::from(user) + ":" + pass;
    String::from("Basic ") + &base64::encode(usrpw.as_bytes())
}

fn add_auth(mut request: ureq::Request) -> ureq::Request {
    if let Ok(api_username) = std::env::var("ANYSNAKE2_GITHUB_API_USERNAME") {
        if let Ok(api_password) = std::env::var("ANYSNAKE2_GITHUB_API_PASSWORD") {
            debug!("Using github auth");
            request = request.set(
                "Authorization",
                &get_basic_auth_header(&api_username, &api_password),
            );
        }
    }
    request
}

impl PyPiDepsDBRetriever {
    fn pypi_deps_db_retrieve(page: i64) -> Result<HashMap<String, String>> {
        let url = format!(
            "https://api.github.com/repos/DavHau/pypi-deps-db/commits?per_page=100&page={}",
            page
        );
        debug!("Retrieving {}", &url);
        let body: String = add_auth(get_proxy_req()?.get(&url)).call()?.into_string()?;
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
        debug!("Retrieved {} entries", res.len());
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
                    trace!(
                        "Could not find entry in pypi-deps-db ({:?} too new)",
                        &self.query_date
                    );
                    page += 1;
                } else {
                    bail!(
                        "Could not find entry in pypi-deps-db (date not present. Closest: {} {}).",
                        pretty_opt_date(&next_smaller_date(&known_mappings, &self.query_date)),
                        pretty_opt_date(&next_larger_date(&known_mappings, &self.query_date)),
                    );
                }
            }
        }
    }
}

fn pretty_opt_date(date: &Option<chrono::NaiveDateTime>) -> String {
    match date {
        Some(x) => x.format("%Y-%m-%d").to_string(),
        None => "".to_string(),
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

fn next_larger_date(
    mappings: &HashMap<String, String>,
    date: &NaiveDateTime,
) -> Option<chrono::NaiveDateTime> {
    let q = date.format("%Y%m%d").to_string();
    let oldest = mappings.keys().filter(|x| *x > &q).min();
    oldest
        .map(|oldest| {
            chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00", oldest), "%Y%m%d %H:%M").ok()
        })
        .flatten()
}
fn next_smaller_date(
    mappings: &HashMap<String, String>,
    date: &NaiveDateTime,
) -> Option<chrono::NaiveDateTime> {
    let q = date.format("%Y%m%d").to_string();
    let oldest = mappings.keys().filter(|x| *x < &q).max();
    oldest
        .map(|oldest| {
            chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00", oldest), "%Y%m%d %H:%M").ok()
        })
        .flatten()
}

fn get_proxy_req() -> Result<ureq::Agent> {
    let mut agent = ureq::AgentBuilder::new();
    let proxy_url = if let Ok(proxy_url) = std::env::var("https_proxy") {
        debug!("found https proxy env var");
        Some(proxy_url)
    } else if let Ok(proxy_url) = std::env::var("http_proxy") {
        debug!("found http proxy env var");
        Some(proxy_url)
    } else {
        None
    };
    if let Some(proxy_url) = proxy_url {
        //let proxy_url = proxy_url
        //.strip_prefix("https://")
        //.unwrap_or_else(|| proxy_url.strip_prefix("http://").unwrap_or(&proxy_url));
        debug!("using proxy_url {}", proxy_url);
        let proxy = ureq::Proxy::new(proxy_url)?;
        agent = agent.proxy(proxy)
    }
    Ok(agent.build())
}

pub fn lookup_github_tag(
    url: &str,
    tag_or_rev: &str,
    flake_dir: impl AsRef<Path>,
) -> Result<String> {
    if tag_or_rev.len() == 40 || !url.starts_with("github:") {
        Ok(tag_or_rev.to_string())
    } else {
        let repo = url.strip_prefix("github:").unwrap();
        fetch_cached(
            &flake_dir
                .as_ref()
                .join(format!(".github_{}.json", repo.replace("/", "_"))),
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

fn fetch_cached(
    cache_filename: impl AsRef<Path>,
    query: &str,
    retriever: impl Retriever,
) -> Result<String> {
    let mut known: HashMap<String, String> = match cache_filename.as_ref().exists() {
        true => serde_json::from_str(&fs::read_to_string(&cache_filename)?)?,
        false => HashMap::new(),
    };
    if known.contains_key(query) {
        return Ok(known.get(query).unwrap().to_string());
    } else {
        let mut new = retriever.retrieve()?;
        for (k, v) in new.drain() {
            known.insert(k, v);
        }
        fs::write(cache_filename, serde_json::to_string_pretty(&json!(known))?)?;
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
            debug!("Retrieving {}", &url);
            let body: String = add_auth(get_proxy_req()?.get(&url)).call()?.into_string()?;
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

fn nix_format(
    input: &str,
    nixpkgs_url: &str,
    nixpkgs_rev: &str,
    flake_dir: impl AsRef<Path>,
) -> Result<String> {
    let full_url = format!("{}?rev={}#nixfmt", nixpkgs_url, nixpkgs_rev);
    super::register_nix_gc_root(&full_url, flake_dir)?;
    let full_args = vec!["shell".to_string(), full_url, "-c".into(), "nixfmt".into()];
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
            "nix fmt error return{}\n{}",
            out.status.code().unwrap(),
            input
        ))
    }
}
