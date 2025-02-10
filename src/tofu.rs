use anyhow::{anyhow, bail, Context, Result};
use std::{collections::HashMap, path::PathBuf, process::Command};
use toml_edit::value;

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::{
    config::{self, SafePythonName, TofuAnysnake2, TofuConfigToml, TofuDevShell, TofuVCSorDev},
    vcs::{self, BranchOrTag, ParsedVCS, TofuVCS},
};
use anysnake2::util::{change_toml_file, get_proxy_req, TomlUpdates};

pub enum PrefetchHashResult {
    Hash(String),
    HaveToUseFetchGit,
}
const NIXPKGS_TAG_REGEX: &str = r"\d\d\.\d\d$";

trait Tofu<A> {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<A>;
}

impl Tofu<config::TofuConfigToml> for config::ConfigToml {
    #[allow(clippy::too_many_lines)]
    fn tofu(self, updates: &mut TomlUpdates) -> Result<config::TofuConfigToml> {
        let converted_clone_regexps = match self.clone_regexps {
            Some(cr) => Some(clone_regex_strings_to_regex(cr)?),
            None => None,
        };
        let parsed_clones: Option<HashMap<String, HashMap<String, ParsedVCS>>> = match self.clones {
            Some(clones) => Some({
                let parsed_clones: Result<_> = clones
                    .into_iter()
                    .map(|(target_folder, entries)| {
                        let parsed_entries: Result<HashMap<String, ParsedVCS>> = entries
                            .into_iter()
                            .map(|(k, v)| {
                                let replaced_v = apply_clone_regexps(&v, &converted_clone_regexps);
                                let parsed_v = ParsedVCS::try_from(replaced_v.as_str()).context(
                            "Failed to parse clone url. Before regex: {v:?}, after: {replaced_v:?}",
                        )?;
                                Ok((k, parsed_v))
                            })
                            .collect();
                        Ok((target_folder, parsed_entries?))
                    })
                    .collect();
                parsed_clones?
            }),
            None => None,
        };

        let python = {
            match self.r {
                Some(_) => {
                    let mut python = self.python.clone();
                    add_rpy2_if_missing(&mut python, updates);
                    python
                }
                None => self.python.clone(),
            }
        };

        let parsed_url: config::TofuVCSorDev = {
            self.anysnake2
                .url2
                .and_then(|x| {
                    <config::ParsedVCSorDev as TryInto<config::TofuVCSorDev>>::try_into(x).ok()
                })
                .expect("Expected to have a completely resolved anysnake2 at this point. Does your anysnake2.toml have a anysnake2.url2 field?")
        };

        let (pre_2_0_url, pre_2_0_rev) = add_pre_2_0_url_and_rev(
            &self.anysnake2.url,
            &self.anysnake2.rev,
            &parsed_url,
            self.anysnake2.use_binary,
            updates,
        );

        Ok(config::TofuConfigToml {
            anysnake2_toml_path: self.anysnake2_toml_path,
            anysnake2: {
                config::TofuAnysnake2 {
                    url: pre_2_0_url,
                    rev: pre_2_0_rev,
                    url2: parsed_url,
                    do_not_modify_flake: self.anysnake2.do_not_modify_flake.unwrap_or(false),
                    dtach: self.anysnake2.dtach,
                }
            },

            nixpkgs: self
                .nixpkgs
                .tofu_to_tag(
                    &["nixpkgs", "url"],
                    updates,
                    "github:NixOS/nixpkgs", // prefer github:/ for then nix doesn't clone the whole
                    // repo..
                    NIXPKGS_TAG_REGEX,
                )?
                .sort_packages(&["nixpkgs", "packages"], updates),
            outside_nixpkgs: self.outside_nixpkgs.tofu_to_tag(
                &["outside_nixpkgs", "url"],
                updates,
                "github:NixOS/nixpkgs",
                NIXPKGS_TAG_REGEX,
            )?, //todo: only tofu newest nixpkgs release.. Doesn't this do this already?
            ancient_poetry: self.ancient_poetry.tofu_to_newest(
                &["ancient_poetry", "url"],
                updates,
                "git+https://codeberg.org/TyberiusPrime/ancient-poetry.git",
            )?,
            uv2nix: self.uv2nix.tofu_to_newest(
                &["uv2nix", "url"],
                updates,
                "github:adisbladis/uv2nix",
            )?,
            uv2nix_override_collection: self.uv2nix_override_collection.tofu_to_newest(
                &["uv2nix_override_collection", "url"],
                updates,
                "github:TyberiusPrime/uv2nix_hammer_overrides",
            )?,
            pyproject_build_systems: self.pyproject_build_systems.tofu_to_newest(
                &["pyproject_build_systems", "url"],
                updates,
                "github:pyproject-nix/build-system-pkgs",
            )?,

            flake_util: self.flake_util.tofu_to_newest(
                &["flake-util", "url"],
                updates,
                "github:numtide/flake-utils",
            )?,
            clone_regexps: converted_clone_regexps,
            clones: match parsed_clones {
                Some(clones) => Some(tofu_clones(clones, updates)?),
                None => None,
            },
            cmd: self.cmd,
            rust: self
                .rust
                .tofu_to_newest(&["rust"], updates, "github:oxalica/rust-overlay")?,
            python: python.tofu(updates)?,
            container: self.container,
            flakes: self.flakes.tofu(updates)?,
            dev_shell: self.dev_shell.tofu(updates)?,
            r: self
                .r
                .tofu_to_newest(&["R"], updates, "github:TyberiusPrime/nixR")?,
        })
    }
}

fn add_rpy2_if_missing(python: &mut Option<config::Python>, _updates: &mut TomlUpdates) {
    // currently this is not feeding back into the anysnake2.toml.
    // One could argue it should, but then we loose the ability to easily patch this on newer
    // versions.
    if let Some(python) = python {
        #[allow(clippy::map_entry)]
        let key = SafePythonName::new("rpy2");
        if !python.packages.contains_key(&key) {
            let source = config::PythonPackageSource::VersionConstraint("".to_string());
            let def = config::PythonPackageDefinition {
                source,
                editable_path: None,
                override_attrs: Default::default(),
                anysnake_override_attrs: None,
                pre_poetry_patch: None,
                build_systems: None,
            };
            python.packages.insert(SafePythonName::new("rpy2"), def);
        }
        let mut overrides = HashMap::new();
        overrides.insert("R_HOME".to_string(), "''${R_tracked}''".to_string());
        overrides.insert(
            "NIX_LDFLAGS".to_string(),
            "''-L${pkgs.bzip2.out}/lib -L${pkgs.xz.out}/lib -L${pkgs.zlib.out}/lib -L${pkgs.icu.out}/lib -L${pkgs.libdeflate}/lib''".to_string(),
        );
        overrides.insert(
            "postPatch".to_string(),
            "''
              substituteInPlace 'rpy2/rinterface_lib/embedded.py' --replace \"os.environ['R_HOME'] = openrlib.R_HOME\" \\
                          \"os.environ['R_HOME'] = openrlib.R_HOME
                      os.environ['R_LIBS_SITE'] = '${R_tracked}/lib/R/library'\"
                ''
            ".to_string(),
        );
        python
            .packages
            .get_mut(&key)
            .unwrap()
            .anysnake_override_attrs = Some(overrides);
    }
}

fn add_pre_2_0_url_and_rev(
    incoming_pre_2_0_url: &Option<String>,
    incoming_rev: &Option<String>,
    parsed_url: &TofuVCSorDev,
    use_binary: Option<bool>,
    updates: &mut TomlUpdates,
) -> (String, String) {
    let use_binary: bool = use_binary.unwrap_or(match parsed_url {
        TofuVCSorDev::Vcs(x) => x.to_string().contains("anysnake2_release_flakes"),
        TofuVCSorDev::Dev => config::Anysnake2::default_use_binary(),
    });

    let pre_2_0_url = incoming_pre_2_0_url.clone().unwrap_or_else(|| {
        (if use_binary {
            "github:TyberiusPrime/anysnake2_release_flakes"
        } else {
            "github:TyberiusPrime/anysnake2"
        })
        .try_into()
        .expect("invalid default url")
    });
    let pre_2_0_rev = match &parsed_url {
        config::TofuVCSorDev::Dev => "dev".to_string(),
        config::TofuVCSorDev::Vcs(vcs) => vcs.get_url_rev_branch().1.to_string(),
    };
    let mut pre_2_0_url_toml = value(pre_2_0_url.to_string());
    pre_2_0_url_toml
        .as_value_mut()
        .unwrap()
        .decor_mut()
        .set_suffix(" # pre 2.0 - 2.0+ uses url2");

    let mut pre_2_0_rev_toml = value(pre_2_0_rev.to_string());
    pre_2_0_rev_toml
        .as_value_mut()
        .unwrap()
        .decor_mut()
        .set_suffix(" # pre 2.0 - 2.0+ uses url2");

    let update_url = match &incoming_pre_2_0_url {
        None => true,
        Some(s) => s != pre_2_0_url.as_str(),
    };
    if update_url {
        updates.push((
            ["anysnake2", "url"]
                .iter()
                .map(ToString::to_string)
                .collect(),
            value(pre_2_0_url_toml.as_value().unwrap()),
        ));
    }

    let update_rev = {
        match &incoming_rev {
            None => true,
            Some(s) => s != pre_2_0_rev.as_str(),
        }
    };
    if update_rev {
        updates.push((
            ["anysnake2", "rev"]
                .iter()
                .map(ToString::to_string)
                .collect(),
            value(pre_2_0_rev_toml.as_value().unwrap()),
        ));
    }
    (pre_2_0_url, pre_2_0_rev)
}

fn clone_regex_strings_to_regex(
    clone_regex: HashMap<String, String>,
) -> Result<Vec<(regex::Regex, String)>> {
    let res: Result<Vec<_>> = clone_regex
        .into_iter()
        .map(|(k, v)| {
            let re = regex::Regex::new(&k)?;
            Ok((re, v))
        })
        .collect();
    res
}

fn apply_clone_regexps(
    input: &str,
    converted_clone_regexps: &Option<Vec<(regex::Regex, String)>>,
) -> String {
    if let Some(converted_clone_regexps) = converted_clone_regexps {
        for (search, replacement) in converted_clone_regexps {
            if search.is_match(input) {
                return search.replace_all(input, replacement).to_string();
            }
        }
    }
    input.to_string()
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

impl TofuToTag<config::TofuNixPkgs> for Option<config::NixPkgs> {
    fn tofu_to_tag(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
        tag_regex: &str,
    ) -> Result<config::TofuNixPkgs> {
        let inner_self = self.unwrap_or_else(config::NixPkgs::new);

        let url_and_rev: vcs::ParsedVCS = inner_self
            .url
            .unwrap_or_else(|| default_url.try_into().unwrap());
        let url_and_rev = tofu_repo_to_tag(
            toml_name,
            updates,
            Some(url_and_rev),
            default_url,
            tag_regex,
        )?;

        let out = config::TofuNixPkgs {
            url: url_and_rev,
            packages: inner_self.packages.unwrap_or_default(),
            allow_unfree: inner_self.allow_unfree,
            permitted_insecure_packages: inner_self.permitted_insecure_packages,
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
        let res = tofu_repo_to_tag(
            toml_name,
            updates,
            self.and_then(|x| x.url),
            default_url,
            tag_regex,
        )?;
        anysnake2::define_outside_nipkgs_url(res.to_nix_string());

        Ok(res)
    }
}

impl TofuToNewest<vcs::TofuVCS> for Option<config::ParsedVCSInsideURLTag> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<vcs::TofuVCS> {
        tofu_repo_to_newest(toml_name, updates, self.and_then(|x| x.url), default_url)
    }
}

impl TofuToNewest<config::TofuUv2Nix> for Option<config::Uv2Nix> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<config::TofuUv2Nix> {
        let prefer_wheels = self.as_ref().and_then(|x| x.prefer_wheels).unwrap_or(true);
        let input = self.and_then(|x| x.url);
        Ok(config::TofuUv2Nix {
            source: tofu_repo_to_newest(toml_name, updates, input, default_url)?,
            prefer_wheels,
        })
    }
}

impl TofuToNewest<Option<config::TofuRust>> for Option<config::Rust> {
    fn tofu_to_newest(
        self,
        toml_name: &[&str],
        updates: &mut TomlUpdates,
        default_url: &str,
    ) -> Result<Option<config::TofuRust>> {
        let mut url_toml_name: Vec<&str> = toml_name.to_vec();
        url_toml_name.push("url");
        Ok(match self {
            None => None,
            Some(rust) => {
                let url = tofu_repo_to_newest(&url_toml_name, updates, rust.url, default_url)?;
                #[allow(clippy::single_match_else)]
                let version = match rust.version {
                    Some(v) => v,
                    None => {
                        debug!("Tofu for rust");
                        let rust_flake_contents = std::process::Command::new("nix")
                            .args(["flake", "show", "--json", &url.to_nix_string()])
                            .output()
                            .with_context(|| format!("nix flake show --json {url} failed"))?;
                        let rust_flake_contents = std::str::from_utf8(&rust_flake_contents.stdout);
                        let json: serde_json::Value = serde_json::from_str(rust_flake_contents?)
                            .context("nix flake show --json wasn't json")?;
                        let rust = json["packages"]["x86_64-linux"]["default"]["name"]
                            .as_str()
                            .context("Could not find default version in flake show")?;
                        let actual_version = rust.split('-').last().context("rust version naming scheme changed, expected something like 'rust-default-1.81.0?'")?;
                        debug!("Found version: {actual_version}");
                        actual_version.to_string()
                    }
                };
                Some(config::TofuRust { version, url })
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
        let mut url_toml_name = toml_name
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>();
        url_toml_name.push("url".to_string());
        let ref_url_toml_name: Vec<&str> = url_toml_name.iter().map(String::as_str).collect();
        Ok(match self {
            None => None,
            Some(inner_self) => {
                let url =
                    tofu_repo_to_newest(&ref_url_toml_name, updates, inner_self.url, default_url)?;
                #[allow(clippy::single_match_else)]
                let date = match inner_self.date {
                    Some(date) => date,
                    None => {
                        let date = find_newest_nixr_date(&url)?;
                        let mut date_toml_name = toml_name
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<String>>();
                        date_toml_name.push("date".to_string());
                        updates.push((date_toml_name, value(date.to_string())));
                        //time to findout
                        date
                    }
                };

                Some(config::TofuR {
                    date,
                    packages: inner_self.packages,
                    url,
                    override_attrs: inner_self.override_attrs,
                    dependency_overrides: inner_self.dependency_overrides,
                    additional_packages: inner_self.additional_packages,
                    use_inside_nix_pkgs: inner_self.use_inside_nix_pkgs,
                })
            }
        })
    }
}

fn find_newest_nixr_date(url: &TofuVCS) -> Result<String> {
    match url {
        TofuVCS::GitHub {
            owner,
            repo,
            branch: _,
            rev,
        } => {
            let url = format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/{rev}/generated/readme.md"
            );
            let text = get_proxy_req()?.get(&url).call()?.into_string()?;
            let date_re = regex::Regex::new(r"(\d{4}-\d{2}-\d{2})")?;
            let mut all_dates = date_re
                .find_iter(&text)
                .map(|x| x.as_str())
                .collect::<Vec<_>>();
            all_dates.sort_unstable();
            let last_date = all_dates
                .last()
                .with_context(|| format!("Could not find dates on {url}"))?;
            Ok((*last_date).to_string())
        }
        _ => {
            bail!("Only know how to determite newest date for R from nixR github, not from other VCS. Add it manually, please");
        }
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
                        &["flakes", &key, "url"],
                        updates,
                        Some(value.url),
                        "",
                    )?;
                    Ok((
                        key,
                        config::TofuFlake {
                            url: tofued,
                            dir: value.dir,
                            follows: value.follows,
                            packages: value.packages,
                        },
                    ))
                })
                .collect(),
        }
    }
}

impl Tofu<config::TofuMinimalConfigToml> for config::MinimalConfigToml {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<config::TofuMinimalConfigToml> {
        let anysnake = match self.anysnake2 {
            Some(value) => value,
            None => config::Anysnake2 {
                //my own cargo version
                rev: Some(env!("CARGO_PKG_VERSION").to_string()),
                url: Some("github:TyberiusPrime/anysnake2_release_flakes".to_string()),
                url2: None,
                use_binary: Some(config::Anysnake2::default_use_binary()),
                do_not_modify_flake: None,
                dtach: config::Anysnake2::default_dtach(),
            },
        };
        let new_url = match anysnake.url2 {
            Some(config::ParsedVCSorDev::Dev) => config::TofuVCSorDev::Dev,
            other => {
                let base = if let Some(true) = anysnake.use_binary {
                    "github:TyberiusPrime/anysnake2_release_flakes"
                } else {
                    "github:TyberiusPrime/anysnake2"
                };

                let url = match other {
                    Some(config::ParsedVCSorDev::Vcs(vcs)) => vcs,
                    Some(_) => unreachable!(),
                    None => base.try_into().expect("invalid default url"),
                };
                let new_url = tofu_repo_to_tag(
                    &["anysnake2", "url2"],
                    updates,
                    Some(url),
                    base,
                    r"(\d\.){1,3}",
                )?;
                add_pre_2_0_url_and_rev(
                    &None,
                    &None,
                    &TofuVCSorDev::Vcs(new_url.clone()),
                    anysnake.use_binary,
                    updates,
                );

                config::TofuVCSorDev::Vcs(new_url)
            }
        };

        Ok(config::TofuMinimalConfigToml {
            anysnake2_toml_path: self.anysnake2_toml_path,
            anysnake2: TofuAnysnake2 {
                url: match &new_url {
                    config::TofuVCSorDev::Vcs(x) => x.get_url_rev_branch().0,
                    config::TofuVCSorDev::Dev => "dev".to_string(),
                },
                rev: match &new_url {
                    config::TofuVCSorDev::Vcs(x) => x.get_url_rev_branch().1.to_string(),
                    config::TofuVCSorDev::Dev => "github:TyberiusPrime/anysnake2".to_string(),
                },
                url2: new_url,
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
    _tofu_repo_to_tag(toml_name, updates, input, default_url, tag_regex).context(error_msg)
}

#[allow(clippy::too_many_lines)]
fn _tofu_repo_to_tag(
    toml_name: &[&str],
    updates: &mut TomlUpdates,
    input: Option<vcs::ParsedVCS>,
    default_url: &str,
    tag_regex: &str,
) -> Result<vcs::TofuVCS> {
    let input = input.unwrap_or_else(|| default_url.try_into().expect("invalid default url"));
    //debug!("tofu_repo_to_tag: {toml_name:?} from {input:?} with /{tag_regex}/");
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
        ParsedVCS::Mercurial {
            url,
            rev: Some(rev),
        } => (
            false,
            TofuVCS::Mercurial {
                url: url.to_string(),
                rev: rev.to_string(),
            },
        ),
        ParsedVCS::Mercurial { url, rev: None } => {
            (
                true,
                TofuVCS::Mercurial {
                    url: url.to_string(),
                    rev: input.newest_revision("")?, //todo: we're ignoring mercurial
                                                     //'branches/bookmarks' for  now
                },
            )
        }
    };
    if changed {
        debug!("changed to {out:?}");
        updates.push((
            toml_name.iter().map(ToString::to_string).collect(),
            value(out.to_string()),
        ));
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
    let (changed, mut newest) =
        _tofu_repo_to_newest(toml_name, updates, &input).context(error_msg)?;

    // workaround for repos that break githubs tar file consistency >
    // todo: this needs to be done once, but even if the user inputs everything, right?
    if changed {
        if let TofuVCS::GitHub {
            owner,
            repo,
            branch: _,
            rev,
        } = &newest
        {
            if let PrefetchHashResult::HaveToUseFetchGit = prefetch_github_hash(owner, repo, rev)? {
                let (url, rev, branch) = newest.get_url_rev_branch();
                warn!("The github repo {owner}/{repo}/?rev={rev} is using .gitattributes and export-subst, which leads to the github tarball used by fetchFromGithub changing hashes over time.\nYour anysnake2.toml has been adjusted to use git directly instead, which is immune to that.");

                newest = TofuVCS::Git {
                    url,
                    branch: branch.to_string(),
                    rev: rev.to_string(),
                };
                updates.push((
                    toml_name.iter().map(ToString::to_string).collect(),
                    value(newest.to_string()),
                ));
            }
        }
    }
    Ok(newest)
}

#[allow(clippy::too_many_lines)]
fn _tofu_repo_to_newest(
    toml_name: &[&str],
    updates: &mut TomlUpdates,
    input: &vcs::ParsedVCS,
) -> Result<(bool, vcs::TofuVCS)> {
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
        ParsedVCS::Mercurial {
            url,
            rev: Some(rev),
        } => (
            false,
            TofuVCS::Mercurial {
                url: url.to_string(),
                rev: rev.to_string(),
            },
        ),
        ParsedVCS::Mercurial { url, rev: None } => {
            (
                true,
                TofuVCS::Mercurial {
                    url: url.to_string(),
                    rev: input.newest_revision("")?, //todo: we're ignoring mercurial
                                                     //'branches/bookmarks' for  now
                },
            )
        }
    };
    if changed {
        //table["url"] = value(out.to_string());
        updates.push((
            toml_name.iter().map(ToString::to_string).collect(),
            value(out.to_string_including_username()),
        ));
    }
    Ok((changed, out))
}

/// apply just enough tofu to get us a toml file.
#[allow(clippy::module_name_repetitions)]
pub fn tofu_anysnake2_itself(
    config: config::MinimalConfigToml,
) -> Result<config::TofuMinimalConfigToml> {
    let config_file = config.anysnake2_toml_path.as_ref().unwrap().clone();
    let mut updates: TomlUpdates = Vec::new();
    let tofued = config.tofu(&mut updates)?;
    if !tofued.anysnake2.do_not_modify_flake {
        change_toml_file(&config_file, updates)?;
    } else if !updates.is_empty() {
        bail!("No anysnake version to use defined in anysnake2.toml, but flake is not allowed to be modified");
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

/* currently unused
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
*/

pub fn prefetch_github_hash(owner: &str, repo: &str, git_hash: &str) -> Result<PrefetchHashResult> {
    let url = format!("https://github.com/{owner}/{repo}/archive/{git_hash}.tar.gz",);

    let stdout = Command::new("nix-prefetch-url")
        .args([&url, "--type", "sha256", "--unpack", "--print-path"])
        .output()
        .context(format!("Failed to nix-prefetch {url}"))?
        .stdout;
    let stdout = std::str::from_utf8(&stdout)?;
    let mut stdout_split = stdout.split('\n');
    let old_format = stdout_split
        .next()
        .with_context(||format!("unexpected output from 'nix-prefetch-url {url} --type sha256 --unpack --print-path' (line 0  - should have been hash)"))?;
    let path = stdout_split
        .next()
        .with_context(||format!("unexpected output from 'nix-prefetch-url {url} --type sha256 --unpack --print-path' (line 1  - should have been hash)"))?;

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

fn get_newest_pypi_version(package_name: &SafePythonName) -> Result<String> {
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

pub fn convert_hash_to_subresource_format(hash: &str) -> Result<String> {
    if hash.is_empty() {
        return Err(anyhow!(
            "convert_hash_to_subresource_format called with empty hash"
        ));
    }
    let res = Command::new("nix")
        .args(["hash", "to-sri", "--type", "sha256", hash])
        .output()
        .context(format!("Failed to nix hash to-sri --type sha256 '{hash}'",))?
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

*/

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
                    let _error_msg =
                        format!("Failed to tofu clone clones.{key1}.{key2} - {value:?}");
                    let inner = _tofu_repo_to_newest(&["clones", &key1, &key2], updates, &value)?;
                    Ok((key2, inner.1))
                })
                .collect::<Result<HashMap<String, TofuVCS>>>()?;
            Ok((key1, outer))
        })
        .collect()
}

impl Tofu<Option<config::TofuPython>> for Option<config::Python> {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<Option<config::TofuPython>> {
        match self {
            Some(inner_self) => {
                let tofu_packages: Result<HashMap<_, _>> = inner_self
                    .packages
                    .into_iter()
                    .map(|(key, value)| {
                        let new = tofu_python_package_definition(&key, &value, updates)
                            .with_context(|| format!("Tofu python package failed: {key}"))?;
                        Ok((key, new))
                    })
                    .collect();

                let date = inner_self.ecosystem_date.unwrap_or_else(|| {
                    //today in yyyy-mm-dd
                    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    let toml_path = vec!["python".to_string(), "ecosystem_date".to_string()];
                    updates.push((toml_path, value(date.clone())));
                    date
                });

                Ok(Some(config::TofuPython {
                    version: inner_self.version,
                    ecosystem_date: date,
                    packages: tofu_packages?,
                    uv_lock_env: inner_self.uv_lock_env,
                }))
            }
            None => Ok(None),
        }
    }
}

#[allow(clippy::enum_glob_use)]
fn tofu_python_package_definition(
    name: &SafePythonName,
    ppd: &config::PythonPackageDefinition,
    updates: &mut TomlUpdates,
) -> Result<config::TofuPythonPackageDefinition> {
    use config::TofuPythonPackageSource::*;
    Ok(config::TofuPythonPackageDefinition {
        editable_path: ppd.editable_path.clone(),
        override_attrs: ppd.override_attrs.clone(),
        anysnake_override_attrs: ppd.anysnake_override_attrs.clone(),
        pre_poetry_patch: ppd.pre_poetry_patch.clone(),
        build_systems: ppd.build_systems.clone(),
        source: match &ppd.source {
            config::PythonPackageSource::VersionConstraint(x) => VersionConstraint(x.to_string()),
            config::PythonPackageSource::Url(x) => Url(x.to_string()),
            config::PythonPackageSource::Vcs(parsed_vcs) => Vcs(tofu_repo_to_newest(
                &["python", "packages", &name.to_string(), "url"],
                updates,
                Some(parsed_vcs.clone()),
                "",
            )?),
            config::PythonPackageSource::PyPi { version } => {
                let pypi_version = match version.as_ref().map(String::as_str) {
                    None | Some("") => get_newest_pypi_version(name)
                        .with_context(|| format!("Could not get pypi version for {name}"))?,
                    Some(version) => version.to_string(),
                };

                let mut out = toml_edit::Table::new().into_inline_table();
                out.insert("version", format!("pypi:{pypi_version}").into());
                let push = pypi_version != version.as_deref().unwrap_or("");

                if push {
                    updates.push((
                        vec![
                            "python".to_string(),
                            "packages".to_string(),
                            name.to_string(),
                        ],
                        value(out),
                    ));
                }

                PyPi {
                    version: pypi_version,
                }
            }
        },
    })
}

impl Tofu<TofuDevShell> for Option<config::DevShell> {
    fn tofu(self, updates: &mut TomlUpdates) -> Result<TofuDevShell> {
        let (inputs, shell) = match self {
            None => {
                updates.push((
                    vec!["dev_shell".to_string(), "inputs".to_string()],
                    value(toml_edit::Array::default()),
                ));

                updates.push((
                    vec!["dev_shell".to_string(), "shell".to_string()],
                    value("bash"),
                ));
                (Vec::new(), "bash".to_string())
            }
            Some(inner_self) => {
                let shell = inner_self.shell.unwrap_or_else(|| {
                    updates.push((
                        vec!["dev_shell".to_string(), "shell".to_string()],
                        value("bash"),
                    ));
                    "bash".to_string()
                });
                let inputs = inner_self.inputs.unwrap_or_else(|| {
                    updates.push((
                        vec!["dev_shell".to_string(), "inputs".to_string()],
                        value(toml_edit::Array::default()),
                    ));
                    Vec::new()
                });

                (inputs, shell)
            }
        };

        let res = TofuDevShell { inputs, shell };

        Ok(res)
    }
}

trait SortPackages {
    fn sort_packages(self, toml_name: &[&str], updates: &mut TomlUpdates) -> Self;
}

impl SortPackages for config::TofuNixPkgs {
    fn sort_packages(self, toml_name: &[&str], updates: &mut TomlUpdates) -> Self {
        let mut out = self.clone();
        out.packages.sort();
        if out.packages != self.packages {
            let op: toml_edit::Array = out.packages.iter().map(ToString::to_string).collect();
            updates.push((
                toml_name.iter().map(ToString::to_string).collect(),
                value(op),
            ));
        }
        out
    }
}
