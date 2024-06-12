use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use std::{borrow::Cow, collections::HashMap, path::PathBuf, process::Command};
use toml_edit::{value, Table};

use log::{debug, info, warn};

use crate::{
    config::{self, BuildPythonPackageInfo, PythonPackageDefinition},
    flake_writer::{self, get_proxy_req},
    python_parsing, run_without_ctrl_c,
    util::{change_toml_file, TomlUpdates},
};

enum PrefetchHashResult {
    Hash(String),
    HaveToUseFetchGit,
}

/// Trust on First use handling
/// if no rev is set, discover it as well
pub fn apply_trust_on_first_use(
    //todo: Where ist the flake stuff?
    config: &mut config::ConfigToml,
    outside_nixpkgs_url: &str,
) -> Result<()> {
    let config_file = config.anysnake2_toml_path.as_ref().unwrap().clone();
    change_toml_file(&config_file, |_doc| {
        let mut updates: TomlUpdates = Vec::new();
        if let Some(python) = config.python.as_mut() {
            apply_trust_on_first_use_python(
                &mut python.packages,
                outside_nixpkgs_url,
                &mut updates,
            )?;
        }
        apply_trust_on_first_use_r(config, &mut updates)?;

        Ok(updates)
    })?;
    Ok(())
}

fn apply_trust_on_first_use_r(
    config: &mut config::ConfigToml,
    updates: &mut TomlUpdates,
) -> Result<()> {
    if let Some(r) = &mut config.r {
        if !r.packages.is_empty() {
            if let None = r.nixr_tag {
                info!("Using discover-newest on first use for nixR");
                let rev = discover_newest_rev_git(&r.nixr_url, Some("main"))?;
                info!("\tDiscovered nixR revision {}", &rev);
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
    python_packages: &mut HashMap<String, PythonPackageDefinition>,
    outside_nixpkgs_url: &str,
    updates: &mut TomlUpdates,
) -> Result<()> {
    if !python_packages.is_empty() {
        for (key, spec) in python_packages.iter_mut() {
            match spec {
                PythonPackageDefinition::Simple(_) | PythonPackageDefinition::Editable(_) => {}
                PythonPackageDefinition::Complex(spec) => {
                    handle_python_git(key, spec, updates).with_context(|| format!("failed on package {key}"))?;
                    handle_python_pypi(key, spec, updates).with_context(|| format!("failed on package {key}"))?;

                    /* let method = spec
                        .get("method")
                        .expect("missing method - should have been caught earlier");
                    let mut hash_key = "".to_string();
                    match &method[..] {
                        "fetchFromGitHub" => {
                            let rev = {
                                match spec.get("rev") {
                                    Some(x) => x.to_string(),
                                    None => {
                                        info!(
                                            "Using discover-newest on first use for python package {}",
                                            key
                                        );
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
                                info!("Using Trust-On-First-Use for python package {}", key);

                                let owner = spec.get("owner").expect("missing owner").to_string();
                                let repo = spec.get("repo").expect("missing repo").to_string();
                                let hash = prefetch_github_hash(&owner, &repo, &rev)?;
                                match hash {
                                    PrefetchHashResult::Hash(hash) => {
                                        debug!("nix-prefetch-hash for {} is {}", key, hash);
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

                                        warn!("The github repo {}/{}/?rev={} is using .gitattributes and export-subst, which leads to the github tarball used by fetchFromGithub changing hashes over time.\nYour anysnake2.toml has been adjusted to use fetchgit instead, which is immune to that.", owner, repo, rev);
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
                                        info!(
                                            "Using discover-newest on first use for python package {}",
                                            key
                                        );
                                        let rev = discover_newest_rev_git(
                                            &url,
                                            spec.get("branchName").map(AsRef::as_ref),
                                        )?;
                                        info!("\tDiscovered revision {}", &rev);
                                        store_rev(spec, updates, key.to_owned(), &rev);
                                        rev
                                    }
                                }
                            };

                            hash_key = format!("hash_{}", rev);
                            if !spec.contains_key(&hash_key) {
                                info!("Using Trust-On-First-Use for python package {}", key);
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
                                        info!(
                                            "Using discover-newest on first use for python package {}",
                                            key
                                        );
                                        let rev = discover_newest_rev_hg(&url)?;
                                        info!("\tDiscovered revision {}", &rev);
                                        store_rev(spec, updates, key.to_owned(), &rev);
                                        rev
                                    }
                                }
                            };
                            hash_key = format!("hash_{}", rev);
                            if !spec.contains_key(&hash_key) {
                                info!("Using Trust-On-First-Use for python package {}", key);
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
                                    info!("Retrieving current version for {} from pypi", key);
                                    let version = get_newest_pipi_version(&pname)?;
                                    store_version(spec, updates, key.to_owned(), &version);
                                    info!("Found version {}", &version);
                                    version
                                }
                            };

                            hash_key = format!("hash_{}", version);
                            if !spec.contains_key(&hash_key) {
                                info!("Using Trust-On-First-Use for python package {}", key);
                                let hash = prefetch_pypi_hash(&pname, &version, outside_nixpkgs_url)
                                    .context("prefetch-pypi-hash")?;
                                store_hash(spec, updates, key.to_owned(), &hash_key, hash);
                                //bail!("bail1")
                            }
                        }
                        _ => {
                            warn!("No trust-on-first-use for method {}, will likely fail with nix hash error!", &method);
                        }
                    };
                    if !hash_key.is_empty() {
                        spec.insert(
                            "sha256".to_string(),
                            spec.get(&hash_key.to_string()).unwrap().to_string(),
                        );
                        spec.retain(|key, _| !key.starts_with("hash_"));
                    } */
                }
            }
        }
    }
    Ok(())
}

fn handle_python_github(
    key: &str,
    spec: &mut toml::map::Map<String, toml::Value>,
    updates: &mut TomlUpdates,
) -> Result<()> {
    if let Some(toml::Value::String(giturl)) = spec.get("github") {
        let owner = spec
            .get("owner")
            .and_then(|x| x.as_str())
            .context("No owner found")?;
    }
    todo!();
}

fn handle_python_git(
    key: &str,
    spec: &mut toml::map::Map<String, toml::Value>,
    updates: &mut TomlUpdates,
) -> Result<()> {
    if let Some(toml::Value::String(giturl)) = spec.get("git") {
        if let None = spec.get("rev") {
            info!(
                "Using discover-newest on first use for python package {}",
                key
            );
            let rev =
                discover_newest_rev_git(&giturl, spec.get("branchName").and_then(|x| x.as_str()))?;
            info!("\tDiscovered revision {}", &rev);
            //spec.retain(|k, _| k == "branchName");
            spec.insert("rev".to_string(), toml::Value::String(rev.clone()));

            store_python_key(spec, updates, key.to_owned(), "rev", rev);
        }
    }
    Ok(())
}
fn handle_python_pypi(
    key: &str,
    spec: &mut toml::map::Map<String, toml::Value>,
    updates: &mut TomlUpdates,
) -> Result<()> {
    if let Some(toml::Value::String(pypi_version)) = spec.get("pypi") {
        let pypi_version = if pypi_version.is_empty() {
            let newest = get_newest_pypi_version(key)?;
            store_python_key(spec, updates, key.to_owned(), "pypi", newest.clone());
            newest

        }    else {
            pypi_version.to_string()
        };



        let url_key = format!("pypi_url_{}", pypi_version);
        if let None = spec.get(&url_key) {
            info!(
                "Using discover-newest on first use for python pypi package {}",
                key
            );
            let url = get_pypi_package_source_url(key, &pypi_version).context(
                "Could not find pypi sdist url",
            )?;
            //spec.insert(url_key.to_string(), toml::Value::String(url));
            store_python_key(spec, updates, key.to_owned(), &url_key, url);

            //let hash = prefetch_pypi_hash(&pypi_version, &pypi_version, "https://nixos.org")?;
            //store_hash(spec, updates, key.to_owned(), &url_key, hash);
        }
        let pypi_file_url = spec
            .get(&url_key)
            .and_then(|x| x.as_str())
            .unwrap()
            .to_string();
        spec.clear();
        spec.insert("url".to_string(), pypi_file_url.into());
    }
    Ok(())
}

/// helper for apply_trust_on_first_use
fn store_python_key(
    spec: &mut toml::map::Map<String, toml::Value>,
    updates: &mut TomlUpdates,
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

    spec.insert(hash_key.to_string(), toml::Value::String(hash.to_owned()));
}

fn store_version(
    spec: &mut BuildPythonPackageInfo,
    updates: &mut TomlUpdates,
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
    debug!("before convert: {}, after: {}", &old_format, &new_format);
    Ok(PrefetchHashResult::Hash(new_format))
}

fn get_pypi_package_source_url(package_name: &str, pypi_version: &str) -> Result<String> {
    let json = get_proxy_req()?
        .get(&format!("https://pypi.org/pypi/{package_name}/json"))
        .call()?
        .into_string()?;
    let json: serde_json::Value = serde_json::from_str(&json)?;
    let files = json["releases"][&pypi_version]
        .as_array()
        .context("No releases found")?;
    for file in files {
        if file["packagetype"] == "sdist" {
            return Ok(file["url"].as_str().context("no url in json")?.to_string());
        }
    }
    bail!("Could not find a sdist release");
}

fn get_newest_pypi_version(package_name: &str) -> Result<String> {
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
pub fn lookup_missing_flake_revs(parsed_config: &mut config::ConfigToml) -> Result<()> {
    if let Some(flakes) = &mut parsed_config.flakes {
        change_toml_file(parsed_config.anysnake2_toml_path.as_ref().unwrap(), |_| {
            let mut updates = TomlUpdates::new();
            for (flake_name, flake) in flakes.iter_mut() {
                if flake.rev.is_none() {
                    if flake.url.starts_with("github:") {
                        use flake_writer::{add_auth, get_proxy_req};
                        let re = Regex::new("github:/?([^/]+)/([^/?]+)/?([^/?]+)?").unwrap();
                        let out = re.captures_iter(&flake.url).next().with_context(|| {
                            format!("Could not parse github url {:?}", flake.url)
                        })?;
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
                        info!(
                            "auto detected head revision for {}: {}",
                            &flake_name,
                            value(commit)
                        );
                        updates.push((
                            vec![
                                "flakes".to_string(),
                                flake_name.to_string(),
                                "rev".to_string(),
                            ],
                            commit.to_string().into(),
                        ));
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
                        info!(
                            "auto detected head revision for {}: {}",
                            &flake_name,
                            value(&rev)
                        );
                        updates.push((
                            vec![
                                "flakes".to_string(),
                                flake_name.to_string(),
                                "rev".to_string(),
                            ],
                            rev.to_string().into(),
                        ));
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
                        info!(
                            "auto detected head revision for {}: {}",
                            &flake_name,
                            value(&rev)
                        );
                        updates.push((
                            vec![
                                "flakes".to_string(),
                                flake_name.to_string(),
                                "rev".to_string(),
                            ],
                            rev.to_string().into(),
                        ));
                        flake.rev = Some(rev);
                    } else {
                        bail!(format!("Flake {} must have a rev (auto lookup of newest rev only supported for 'github:' or 'hg+https://' hosted flakes", flake_name));
                    }
                }
            }
            Ok(updates)
        })?;
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
