use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use regex::Regex;
use std::{borrow::Cow, collections::HashMap, hash::Hash, path::PathBuf, process::Command};
use toml_edit::{value, Item, Table};
use version_compare::Version;

use log::{debug, info, warn};

use crate::{
    config::{
        self, BuildPythonPackageInfo, MinimalConfigToml, NixPkgs, PythonPackageDefinition,
        TofuAnysnake2, TofuConfigToml,
    },
    flake_writer::{self, get_proxy_req},
    python_parsing, run_without_ctrl_c,
    util::{change_toml_file, TomlUpdates},
    vcs::{self, run_git_ls, BranchOrTag, ParsedVCS, TofuVCS},
};

enum PrefetchHashResult {
    Hash(String),
    HaveToUseFetchGit,
}
const NIXPKGS_TAG_REGEX: &str = r"\d\d\.\d\d$";

trait Tofu<A> {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<A>;
}

impl Tofu<config::TofuConfigToml> for config::ConfigToml {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<config::TofuConfigToml> {
        Ok(config::TofuConfigToml {
            anysnake2_toml_path: self.anysnake2_toml_path,
            anysnake2: {
                config::TofuAnysnake2 {
                    url: {
                        debug!("So far {:?}", self.anysnake2.url);
                        self.anysnake2.url.and_then(|x| x.try_into().ok()).expect(
                            "Expected to have a completely resolved anysnake2 at this point",
                        )
                    },
                    use_binary: self.anysnake2.use_binary,
                    do_not_modify_flake: self.anysnake2.do_not_modify_flake.unwrap_or(false),
                    dtach: self.anysnake2.dtach,
                }
            },
            nixpkgs: self.nixpkgs.tofu_to_tag(
                &["nixpkgs"],
                updates,
                "github:NixOS/nixpkgs", // prefer github:/ for then nix doesn't clone the whole
                // repo..
                NIXPKGS_TAG_REGEX,
            )?,
            outside_nixpkgs: self.outside_nixpkgs.tofu_to_tag(
                &["outside_nixpkgs"],
                updates,
                "github:NixOS/nixpkgs",
                NIXPKGS_TAG_REGEX,
            )?, //todo: only tofu newest nixpkgs release..
            ancient_poetry: self.ancient_poetry.tofu_to_newest(
                &["ancient_poetry"],
                updates,
                "git+https://codeberg.org/TyberiusPrime/ancient-poetry.git",
            )?,
            poetry2nix: self.poetry2nix.tofu_to_newest(
                &["poetry2nix"],
                updates,
                "github:nix-community/poetry2nix",
            )?,
            flake_util: self.flake_util.tofu_to_newest(
                &["flake-util"],
                updates,
                "github:numtide/flake-utils",
            )?,
            clone_regexps: self.clone_regexps,
            clones: match self.clones {
                Some(clones) => Some(tofu_clones(clones, updates)?),
                None => None,
            },
            cmd: self.cmd,
            rust: self
                .rust
                .tofu_to_newest(&["rust"], updates, "github:oxalica/rust-overlay")?,
            python: self.python,
            container: self.container,
            flakes: self.flakes.tofu(updates)?,
            dev_shell: self.dev_shell,
            r: self
                .r
                .tofu_to_newest(&["R"], updates, "github:TyberiusPrime/nixR")?,
        })
    }
}

trait TofuToNewest<A> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<A>;
}

trait TofuToTag<A> {
    fn tofu_to_tag(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
        tag_regex: &str,
    ) -> Result<A>;
}

impl TofuToTag<config::TofuNixpkgs> for Option<config::NixPkgs> {
    fn tofu_to_tag(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
        tag_regex: &str,
    ) -> Result<config::TofuNixpkgs> {
        let _self = self.unwrap_or_else(|| config::NixPkgs::new());

        let url_and_rev: vcs::ParsedVCS =
            _self.url.unwrap_or_else(|| default_url.try_into().unwrap());
        let url_and_rev = tofu_repo_to_tag(
            toml_name,
            updates,
            Some(url_and_rev),
            default_url,
            tag_regex,
        )?;

        let mut out = config::TofuNixpkgs {
            url: url_and_rev,
            packages: _self.packages.unwrap_or_else(|| Vec::new()),
            allow_unfree: _self.allow_unfree,
        };
        Ok(out)
    }
}

impl TofuToTag<vcs::TofuVCS> for Option<config::ParsedVCSInsideURLTag> {
    fn tofu_to_tag(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
        tag_regex: &str,
    ) -> Result<vcs::TofuVCS> {
        Ok(tofu_repo_to_tag(
            toml_name,
            updates,
            self.map(|x|x.url),
            default_url,
            tag_regex,
        )?)
    }
}

impl TofuToNewest<vcs::TofuVCS> for Option<config::ParsedVCSInsideURLTag> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<vcs::TofuVCS> {
        Ok(tofu_repo_to_newest(toml_name, updates, self.map(|x|x.url), default_url)?)
    }
}

impl TofuToNewest<Option<config::TofuRust>> for Option<config::Rust> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<Option<config::TofuRust>> {
        Ok(match self {
            None => None,
            Some(rust) => {
                if rust.version.is_none() {
                    bail!("When using rust, you must specify a version");
                }
                Some(config::TofuRust {
                    version: rust.version,
                    url: tofu_repo_to_newest(toml_name, updates, rust.url, default_url)?,
                })
            }
        })
    }
}

impl TofuToNewest<Option<config::TofuR>> for Option<config::R> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<Option<config::TofuR>> {
        Ok(match self {
            None => None,
            Some(_self) => Some(config::TofuR {
                date: _self.date,
                packages: _self.packages,
                url: tofu_repo_to_newest(toml_name, updates, _self.url, default_url)?,
                override_attrs: _self.override_attrs,
                dependency_overrides: _self.dependency_overrides,
                additional_packages: _self.additional_packages,
            }),
        })
    }
}

impl Tofu<config::Python> for config::Python {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<config::Python> {
        //    python_packages: &mut HashMap<String, PythonPackageDefinition>,
        //   outside_nixpkgs_url: &str,
        //  updates: &mut TomlUpdates,
        let mut new_packages: HashMap<String, config::PythonPackageDefinition> = HashMap::new();
        for (key, spec) in self.packages.iter() {
            let new = match spec {
                PythonPackageDefinition::Simple(_) | PythonPackageDefinition::Editable(_) => {
                    spec.clone()
                }
                PythonPackageDefinition::Complex(spec) => {
                    let mut new = spec.clone();
                    handle_python_git(key, &mut new, updates)
                        .with_context(|| format!("failed on package {key}"))?;
                    handle_python_pypi(key, &mut new, updates)
                        .with_context(|| format!("failed on package {key}"))?;
                    PythonPackageDefinition::Complex(new)

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
            };
            new_packages.insert(key.to_string(), new);
        }
        Ok(config::Python {
            version: self.version,
            ecosystem_date: self.ecosystem_date,
            packages: new_packages,
        })
    }
}

impl Tofu<HashMap<String, config::TofuFlake>> for Option<HashMap<String, config::Flake>> {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<HashMap<String, config::TofuFlake>> {
        match self {
            None => Ok(HashMap::new()),
            Some(flakes) => flakes
                .into_iter()
                .map(|(key, value)| {
                    let tofued = tofu_repo_to_newest(
                        &["flakes", &key],
                        updates,
                        Some(value.url),
                        "",
                    )?;
                    Ok((
                        key,
                        config::TofuFlake {
                            url: tofued,
                            follows: value.follows,
                            packages: value.packages.unwrap_or_else(|| Vec::new()),
                        },
                    ))
                })
                .collect(),
        }
    }
}

impl Tofu<config::TofuMinimalConfigToml> for config::MinimalConfigToml {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<config::TofuMinimalConfigToml> {
        let mut anysnake = match self.anysnake2 {
            Some(value) => value,
            None => config::Anysnake2 {
                url: None,
                use_binary: config::Anysnake2::default_use_binary(),
                do_not_modify_flake: None,
                dtach: config::Anysnake2::default_dtach(),
            },
        };
        let new_url = match anysnake.url {
            Some(config::ParsedVCSorDev::Dev) => config::TofuVCSorDev::Dev,
            other => {
                let url = match other {
                    Some(config::ParsedVCSorDev::VCS(vcs)) => vcs,
                    Some(_) => unreachable!(),
                    None => "github:TyberiusPrime/anysnake2"
                        .try_into()
                        .expect("invalid default url"),
                };
                let new_url = tofu_repo_to_tag(
                    &["anysnake2"],
                    updates,
                    Some(url),
                    if anysnake.use_binary {
                        "github:TyberiusPrime/anysnake2_release_flakes"
                    } else {
                        "github:TyberiusPrime/anysnake2"
                    },
                    r"(\d\.){1,3}",
                )?;
                config::TofuVCSorDev::VCS(new_url)
            }
        };

        Ok(config::TofuMinimalConfigToml {
            anysnake2_toml_path: self.anysnake2_toml_path,
            anysnake2: TofuAnysnake2 {
                url: new_url,
                use_binary: anysnake.use_binary,
                do_not_modify_flake: anysnake.do_not_modify_flake.unwrap_or(false),
                dtach: anysnake.dtach,
            },
        })
    }
}

fn tofu_repo_to_tag(
    toml_name: &[&str],
    updates: &mut TomlUpdates,
    input: Option<vcs::ParsedVCS>,
    default_url: &str,
    tag_regex: &str,
) -> Result<vcs::TofuVCS> {
    let error_msg = format!("Trust-on-first-use-failed on {input:?}. Default url: {default_url}");
    Ok(_tofu_repo_to_tag(toml_name, updates, input, default_url, tag_regex).context(error_msg)?)
}

fn _tofu_repo_to_tag(
    toml_name: &[&str],
    updates: &mut TomlUpdates,
    input: Option<vcs::ParsedVCS>,
    default_url: &str,
    tag_regex: &str,
) -> Result<vcs::TofuVCS> {
    let input = input.unwrap_or_else(|| default_url.try_into().expect("invalid default url"));
    let (changed, out) = match &input {
        vcs::ParsedVCS::Git {
            url,
            branch: Some(branch),
            rev: Some(rev),
        } => (
            false,
            vcs::TofuVCS::Git {
                url: url.to_string(),
                branch: branch.to_string(),
                rev: rev.to_string(),
            },
        ),
        vcs::ParsedVCS::Git {
            url,
            branch: None,
            rev: Some(rev),
        } => (
            true,
            vcs::TofuVCS::Git {
                url: url.to_string(),
                branch: input.discover_main_branch()?,
                rev: rev.to_string(),
            },
        ),

        vcs::ParsedVCS::Git {
            url,
            branch: _,
            rev: None,
        } => {
            //branch is irrelevant, rev is now missing.
            let branch = input.discover_main_branch()?;
            let rev = input.newest_tag(tag_regex)?;
            (
                true,
                vcs::TofuVCS::Git {
                    url: url.to_string(),
                    branch,
                    rev,
                },
            )
        }
        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch: Some(branch),
            rev: Some(rev),
        } => (
            false,
            vcs::TofuVCS::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: branch.to_string(),
                rev: rev.to_string(),
            },
        ),

        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch: Some(branch),
            rev: None,
        } => {
            // branch could either be a branch. Or a tag. We have to discover it.
            match input.branch_or_tag(branch)? {
                BranchOrTag::Branch => (
                    true,
                    vcs::TofuVCS::GitHub {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        branch: branch.to_string(),
                        rev: input.newest_tag(tag_regex)?,
                    },
                ),
                BranchOrTag::Tag => (
                    true,
                    vcs::TofuVCS::GitHub {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        branch: input.discover_main_branch()?,
                        rev: branch.to_string(),
                    },
                ),
            }
        }
        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch: None,
            rev: None,
        } => (
            true,
            vcs::TofuVCS::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: input.discover_main_branch()?,
                rev: input.newest_tag(tag_regex)?,
            },
        ),
        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch: None,
            rev: Some(rev),
        } => (
            true,
            vcs::TofuVCS::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: input.discover_main_branch()?,
                rev: rev.to_string(),
            },
        ),
    };
    if changed {
        let mut table = toml_edit::table();
        table["url"] = value(out.to_string());
        updates.push((toml_name.iter().map(ToString::to_string).collect(), table));
    }
    Ok(out)
}

fn tofu_repo_to_newest(
    toml_name: &[&str],
    updates: &mut TomlUpdates,
    input: Option<vcs::ParsedVCS>,
    default_url: &str,
) -> Result<vcs::TofuVCS> {
    let input = input.unwrap_or_else(|| default_url.try_into().expect("invalid default url"));
    let error_msg = format!("Trust-on-first-use-failed on {input:?}. Default url: {default_url}");
    Ok(_tofu_repo_to_newest(toml_name, updates, input).context(error_msg)?)
}

fn _tofu_repo_to_newest(
    toml_name: &[&str],
    updates: &mut TomlUpdates,
    input: vcs::ParsedVCS,
) -> Result<vcs::TofuVCS> {
    let (changed, out) = match &input {
        vcs::ParsedVCS::Git {
            url,
            branch: Some(branch),
            rev: Some(rev),
        } => (
            false,
            vcs::TofuVCS::Git {
                url: url.to_string(),
                branch: branch.to_string(),
                rev: rev.to_string(),
            },
        ),
        vcs::ParsedVCS::Git {
            url,
            branch: None,
            rev: Some(rev),
        } => (
            true,
            vcs::TofuVCS::Git {
                url: url.to_string(),
                branch: input.discover_main_branch()?,
                rev: rev.to_string(),
            },
        ),
        vcs::ParsedVCS::Git {
            url,
            branch,
            rev: None,
        } => {
            let branch = branch
                .as_deref()
                .map(ToString::to_string)
                .unwrap_or(input.discover_main_branch()?);

            (
                true,
                vcs::TofuVCS::Git {
                    url: url.to_string(),
                    rev: input.newest_revision(&branch)?,
                    branch,
                },
            )
        }
        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch: Some(branch),
            rev: Some(rev),
        } => (
            false,
            vcs::TofuVCS::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: branch.to_string(),
                rev: rev.to_string(),
            },
        ),
        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch,
            rev: None,
        } => {
            let branch = branch
                .as_deref()
                .map(ToString::to_string)
                .unwrap_or(input.discover_main_branch()?);

            (
                true,
                vcs::TofuVCS::GitHub {
                    owner: owner.to_string(),
                    repo: repo.to_string(),
                    branch: branch.to_string(),
                    rev: input.newest_revision(&branch)?,
                },
            )
        }
        vcs::ParsedVCS::GitHub {
            owner,
            repo,
            branch: None,
            rev: Some(rev),
        } => (
            true,
            vcs::TofuVCS::GitHub {
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: input.discover_main_branch()?,
                rev: rev.to_string(),
            },
        ),
    };
    if changed {
        //table["url"] = value(out.to_string());
        updates.push((toml_name.iter().map(ToString::to_string).collect(), value(out.to_string())));
    }
    Ok(out)
}

/// aply just enough tofu to get us a toml file.
pub fn tofu_anysnake2_itself(
    config: config::MinimalConfigToml,
) -> Result<config::TofuMinimalConfigToml> {
    let config_file = config.anysnake2_toml_path.as_ref().unwrap().clone();
    let mut updates: TomlUpdates = Vec::new();
    let tofued = config.tofu(&mut updates)?;
    if !tofued.anysnake2.do_not_modify_flake {
        change_toml_file(&config_file, updates)?;
    } else {
        if !updates.is_empty() {
            bail!("No anysnake version to use defined in anysnake2.toml, but flake is not allowed to be modified");
        }
    }
    Ok(tofued)
}

/// Trust on First use handling
/// if no rev is set, discover it as well
pub fn apply_trust_on_first_use(
    //todo: Where ist the flake stuff?
    config: config::ConfigToml,
) -> Result<TofuConfigToml> {
    let config_file = config.anysnake2_toml_path.as_ref().unwrap().clone();
    let mut updates: TomlUpdates = Vec::new();
    let tofued = config.tofu(&mut updates)?;
    change_toml_file(&config_file, updates)?;
    Ok(tofued)
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
        } else {
            pypi_version.to_string()
        };

        let url_key = format!("pypi_url_{}", pypi_version);
        if let None = spec.get(&url_key) {
            info!(
                "Using discover-newest on first use for python pypi package {} {spec:?}",
                key
            );
            let url = get_pypi_package_source_url(key, &pypi_version)
                .context("Could not find pypi sdist url")?;
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
        spec.retain(|k, _| !k.starts_with("pypi"));
        spec.insert("url".to_string(), pypi_file_url.clone().into());
        updates.push((
            vec![
                "python".to_string(),
                "packages".to_string(),
                key.to_owned(),
                url_key,
            ],
            Item::Value(pypi_file_url.into()),
        ));
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
        Item::Value(hash.clone().into()),
    ));

    spec.insert(hash_key.to_string(), toml::Value::String(hash.into()));
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
        Item::Value(version.into()),
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
/* pub fn lookup_missing_flake_revs(parsed_config: &mut config::ConfigToml) -> Result<()> {
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
                            Item::Value(commit.into()),
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
                            Item::Value(rev.to_string().into()),
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
                            Item::Value(rev.to_string().into()),
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
} */

fn rewrite_github_url(url: &str, branch: Option<&str>) -> (String, Option<String>) //todo cow
{
    let rewritten_url = if url.starts_with("github:") {
        url.replace("github:", "https://github.com/")
    } else {
        url.to_string()
    };
    let (rewritten_url, branch): (String, Option<String>) = match branch {
        Some(branch) => (rewritten_url, Some(branch.to_string())),
        None => {
            if rewritten_url.contains("?ref=") {
                let (rewritten_url, new_branch) = rewritten_url.rsplit_once("?ref=").unwrap();
                info!(
                    "auto detected branch {} from url: {}",
                    new_branch, rewritten_url
                );
                (rewritten_url.to_string(), Some(new_branch.to_string()))
            } else {
                (rewritten_url, None)
            }
        }
    };
    (rewritten_url, branch)
}

fn discover_newest_rev_git(url: &str, branch: Option<&str>) -> Result<String> {
    let (rewritten_url, branch) = rewrite_github_url(url, branch);

    let refs = match &branch {
        Some(x) => format!("refs/heads/{}", x),
        None => "HEAD".to_string(),
    };
    let hashes = run_git_ls(&rewritten_url, Some(refs.as_str()))?;
    if hashes.is_empty() {
        bail!(
            "Could not find revision hash in 'git ls-remote {} {}' output.{}",
            url,
            refs,
            if branch.is_some() {
                " Is your branchName correct?"
            } else {
                ""
            }
        );
    } else {
        Ok(hashes[0].0.clone())
    }
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

fn tofu_clones(
    clones: HashMap<String, HashMap<String, ParsedVCS>>,
    updates: &mut TomlUpdates,
) -> Result<HashMap<String, HashMap<String, TofuVCS>>> {
    clones
        .into_iter()
        .map(|(key1, value)| {
            let outer = value
                .into_iter()
                .map(|(key2, value)| {
                    let error_msg =
                        format!("Failed to tofu clone clones.{key1}.{key2} - {value:?}");
                    let inner =
                        _tofu_repo_to_newest(&["clones", &key1, &key2], updates, value)?;
                    Ok((key2, inner))
                })
                .collect::<Result<HashMap<String, TofuVCS>>>()?;
            Ok((key1, outer))
        })
        .collect()
}


