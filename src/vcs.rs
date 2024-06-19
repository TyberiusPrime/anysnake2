use std::{borrow::Cow, collections::HashMap};

use anyhow::{bail, Context, Result};
use log::{debug, error};
use serde::Serialize;
use version_compare::Version;

use crate::{flake_writer::add_auth, run_without_ctrl_c, util::get_proxy_req};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ParsedVCS {
    Git {
        url: String,
        branch: Option<String>,
        rev: Option<String>,
    },
    GitHub {
        owner: String,
        repo: String,
        branch: Option<String>,
        rev: Option<String>,
    },
}

#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
pub enum TofuVCS {
    Git {
        url: String,
        branch: String,
        rev: String,
    },
    GitHub {
        owner: String,
        repo: String,
        branch: String,
        rev: String,
    },
}

impl TofuVCS {
    pub fn to_nix_string(&self) -> String {
        match self {
            TofuVCS::Git { url, branch, rev } => {
                format!("git+{url}?ref={branch}&rev={rev}")
            }
            TofuVCS::GitHub {
                owner,
                repo,
                branch: _,
                rev,
            } => {
                format!("github:{owner}/{repo}/{rev}")
            }
        }
    }

    pub fn get_url_rev_branch(&self) -> (String, &str, &str) {
        match self {
            TofuVCS::Git { url, branch, rev } => (url.to_string(), rev, branch),
            TofuVCS::GitHub {
                owner,
                repo,
                branch,
                rev,
            } => (
                format!("https://github.com/{owner}/{repo}.git"),
                rev,
                branch,
            ),
        }
    }

    pub fn clone_repo(&self, target_dir: &str) -> Result<()> {
        let (url, rev, branch) = self.get_url_rev_branch();
        //let clone_args =
        run_without_ctrl_c(|| {
            let inner = || {
                let mut proc = std::process::Command::new("git");
                proc.args(["clone", &url, target_dir]);
                debug!("Running {:?}", proc);
                let status = proc
                    .status()
                    .with_context(|| format!("Git clone failed for {self}"))?;
                if !status.success() {
                    bail!("Git clone failed for {self}");
                }

                let mut proc = std::process::Command::new("git");
                proc.args(["checkout", branch]);
                proc.current_dir(target_dir);
                debug!("Running {:?}", proc);
                let status = proc
                    .status()
                    .with_context(|| format!("Git checkout failed for {self}"))?;
                if !status.success() {
                    bail!("Git checkout failed for {self}");
                }
                //git reset
                let mut proc = std::process::Command::new("git");
                proc.args(["reset", "--hard", rev]);
                proc.current_dir(target_dir);
                debug!("Running {:?}", proc);
                let status = proc
                    .status()
                    .with_context(|| format!("Git reset failed for {self}"))?;
                if !status.success() {
                    bail!("Git reset failed for {self}");
                }
                Ok(())
            };

            if let Err(msg) = inner() {
                error!("Throwing away cloned repo because of error: {msg:?}");
                ex::fs::remove_dir_all(target_dir)
                    .context("Failed to remove target dir of failed clone")?;

                return Err(msg);
            }

            Ok(())
        })?;

        Ok(())
    }
}

impl std::fmt::Display for TofuVCS {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            TofuVCS::Git { url, branch, rev } => format!("git+{url}?ref={branch}&rev={rev}"),
            TofuVCS::GitHub {
                owner,
                repo,
                branch,
                rev,
            } => format!("github:{owner}/{repo}/{branch}/{rev}"),
        })
    }
}

pub enum BranchOrTag {
    Branch,
    Tag,
}

impl TryFrom<&str> for ParsedVCS {
    type Error = anyhow::Error;

    /// Parse from a nix-like url, but not supporting the flake registry..
    ///
    /// Like the examples from the nix manual, we parse
    /// - `github:NixOS/nixpkgs`: The master branch of the NixOS/nixpkgs repository on GitHub.
    /// - `github:NixOS/nixpkgs/nixos-20.09`: The nixos-20.09 branch of the nixpkgs repository.
    /// - `github:NixOS/nixpkgs/a3a3dda3bacf61e8a39258a0ed9c924eeca8e293`: A specific revision of the nixpkgs repository.
    /// - `github:edolstra/nix-warez?dir=blender`: A flake in a subdirectory of a GitHub repository.
    /// - `git+https://github.com/NixOS/patchelf`: A Git repository.
    /// - `git+https://github.com/NixOS/patchelf?ref=master`: A specific branch of a Git repository.
    /// - `git+https://github.com/NixOS/patchelf?ref=master&rev=f34751b88bd07d7f44f5cd3200fb4122bf916c7e`: A specific branch and revision of a Git repository.
    ///
    /// In addition we understand:
    /// - github:NixOS/nixpkgs//24.05: The 24.05 *tag* of that repo (empty branch...)
    /// - `git+https://github.com/NixOS/patchelf?rev=4.05`: A specific branch of a Git repository.
    ///  - `github:NixOS/patchelf/master/f34751b88bd07d7f44f5cd3200fb4122bf916c7e` to be the specific branch and revision of a Github repository.
    ///    (that's mostly a 'we ignore the branch', but it's useful so you can strip of the tag and
    ///    get the newest from that branch tofued)

    fn try_from(input: &str) -> Result<Self> {
        Ok(if input.starts_with("git+") {
            let url = input.strip_prefix("git+").unwrap();
            let mut parts = url.splitn(2, '?');
            let url = parts.next().unwrap();
            let query_string = extract_query_string(parts.next().unwrap_or_default())?;
            let branch = query_string.get("ref").map(ToString::to_string);
            let rev = query_string.get("rev").map(ToString::to_string);
            for k in query_string.keys() {
                if k != "ref" && k != "rev" {
                    bail!("Unknown query string key: {}", k);
                }
            }
            ParsedVCS::Git {
                url: url.to_string(),
                branch,
                rev,
            }
        } else if input.starts_with("github:") {
            let mut parts = input.splitn(4, '/');
            let owner = parts
                .next()
                .unwrap()
                .strip_prefix("github:")
                .unwrap()
                .to_string();
            let repo = parts
                .next()
                .context("No repo in github:owner/repo url definition")?
                .to_string();
            let mut branch = parts.next().map(ToString::to_string);
            if branch == Option::Some(String::new()) {
                branch = None;
            }
            let mut rev = parts.next().map(ToString::to_string);
            if let Some(inner_branch) = &branch {
                if could_be_a_sha1(inner_branch) && rev.is_none() {
                    rev = branch;
                    branch = None;
                }
            }
            ParsedVCS::GitHub {
                owner,
                repo,
                branch,
                rev,
            }
        } else if input.starts_with("path:/") {
            bail!("flake urls must not start with path:/. These handle ?rev= wrong. Use just an absolute path instead");
        } else {
            bail!("unknown vcs: {}", input);
        })
    }
}

impl ParsedVCS {
    fn get_tags(&self) -> Result<HashMap<String, String>> {
        match self {
            ParsedVCS::Git {
                url,
                branch: _branch,
                rev: _rev,
            } => {
                let hash_and_ref = run_git_ls(url, None)?;
                let res: Result<_> = hash_and_ref
                    .into_iter()
                    .filter_map(|(hash, refname)| {
                        refname
                            .strip_prefix("refs/tags/")
                            .map(|tag| Ok((tag.to_string(), hash)))
                    })
                    .collect();
                Ok(res?)
            }
            ParsedVCS::GitHub {
                owner,
                repo,
                branch: _,
                rev: _rev,
            } => {
                //should be run git_ls instead?
                let mut res = HashMap::new();
                for page in 0..30 {
                    let json = get_github_tags(owner, repo, page)?;
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
    }

    pub fn newest_tag(&self, tag_regex: &str) -> Result<String> {
        let tags = self.get_tags()?;
        let search_re =
            regex::Regex::new(tag_regex).expect("failed to parse tag regex, coding error");
        let matches: Result<Vec<_>> = tags
            .iter()
            .filter(|(refname, _hash)| search_re.is_match(refname))
            .map(|(refname, hash)| {
                Ok((
                    Version::from(refname).with_context(|| {
                        format!("Could not parse tag/version for ordering: {refname}")
                    })?,
                    hash,
                    refname.to_string(),
                ))
            })
            .collect();
        let mut matches = matches?;
        matches.sort_by(
            |(version_a, _hash_a, _refname_a), (version_b, _hash_b, _refname_b)| {
                version_b.compare(version_a).ord().unwrap() //doc says unwrap doesn't fail
            },
        );
        if matches.is_empty() {
            bail!("Could not find any tag matching the regexp /{tag_regex}/. Found tags: {tags:?}");
        }
        Ok(matches[0].2.clone())
    }

    pub fn branch_or_tag(&self, query: &str) -> Result<BranchOrTag> {
        let temp = run_git_ls(&self.get_git_url(), Some(&format!("refs/heads/{query}")))?;
        if temp.is_empty() {
            Ok(BranchOrTag::Tag)
        } else {
            Ok(BranchOrTag::Branch)
        }
    }

    fn get_branches(&self) -> Result<Vec<String>> {
        let hash_and_ref = match self {
            ParsedVCS::Git {
                url,
                branch: _branch,
                rev: _rev,
            } => run_git_ls(url, None)?,
            ParsedVCS::GitHub {
                owner,
                repo,
                branch: _,
                rev: _rev,
            } => {
                let url = format!("https://github.com/{owner}/{repo}.git");
                run_git_ls(&url, None)?
            }
        };
        let res: Vec<String> = hash_and_ref
            .into_iter()
            .filter_map(|(_hash, refname)| {
                refname
                    .strip_prefix("refs/heads/")
                    .map(ToString::to_string)
            })
            .collect();
        Ok(res)
    }

    pub fn discover_main_branch(&self) -> Result<String> {
        let branches = self.get_branches()?;
        if branches.iter().any(|x| x == "main") {
            Ok("main".to_string())
        } else if branches.iter().any(|x| x == "master") {
            Ok("master".to_string())
        } else if branches.len() == 1 {
            Ok(branches[0].clone())
        } else {
            Err(anyhow::anyhow!(
                "No main or master branch found. You have to specify the main branch yourself. Found: {branches:?}",
            ))
        }
    }

    fn get_git_url(&self) -> Cow<str> {
        match self {
            ParsedVCS::Git {
                url,
                branch: _,
                rev: _,
            } => Cow::Borrowed(url),
            ParsedVCS::GitHub {
                owner,
                repo,
                branch: _,
                rev: _,
            } => Cow::Owned(format!("https://github.com/{owner}/{repo}")),
        }
    }

    pub fn newest_revision(&self, branch: &str) -> Result<String> {
        let hash_and_ref = run_git_ls(&self.get_git_url(), Some(branch))?;

        if hash_and_ref.is_empty() {
            bail!(
                "No revisions found for git url {:?}, branch: {}",
                self,
                branch
            );
        }
        Ok(hash_and_ref[0].0.clone())
    }
}

impl TryFrom<ParsedVCS> for TofuVCS {
    type Error = anyhow::Error;

    fn try_from(value: ParsedVCS) -> Result<TofuVCS> {
        Ok(match value {
            ParsedVCS::Git { url, branch, rev } => TofuVCS::Git {
                url,
                branch: branch.ok_or_else(|| anyhow::anyhow!("No branch in git url"))?,
                rev: rev.ok_or_else(|| anyhow::anyhow!("No rev in git url"))?,
            },
            ParsedVCS::GitHub {
                owner,
                repo,
                branch,
                rev,
            } => TofuVCS::GitHub {
                owner,
                repo,
                branch: branch.ok_or_else(|| anyhow::anyhow!("No branch in github url"))?,
                rev: rev.ok_or_else(|| anyhow::anyhow!("No rev in github url"))?,
            },
        })
    }
}

pub fn run_git_ls(url: &str, branch: Option<&str>) -> Result<Vec<(String, String)>> {
    let url = url.strip_prefix("git+").unwrap_or(url);
    debug!("Running git ls remote on {}, branch: {:?}", url, branch);
    let output = run_without_ctrl_c(|| {
        //todo: run this is in the provided nixpkgs!
        let mut proc = std::process::Command::new("git");
        proc.args(["ls-remote", url]);
        if let Some(branch) = branch {
            proc.arg(branch);
        }
        Ok(proc.output()?)
    })
    .context("git ls-remote failed")?;
    let stdout =
        std::str::from_utf8(&output.stdout).expect("utf-8 decoding failed  no hg id --debug");
    let mut res = Vec::new();
    for line in stdout.lines() {
        let (hash, refname) = line
            .split_once('\t')
            .context("no tab in git ls-remote output")?;
        res.push((hash.to_string(), refname.to_string()));
    }
    Ok(res)
}

pub fn extract_query_string(input: &str) -> Result<HashMap<String, String>> {
    let mut res = HashMap::new();
    if !input.is_empty() {
        for kv_pair in input.split('&') {
            let (k, v) = kv_pair.split_once('=').context("no = in query string")?;
            res.insert(k.to_string(), v.to_string());
        }
    }
    Ok(res)
}

pub(crate) fn get_github_tags(
    owner: &str,
    repo: &str,
    page: i32,
) -> Result<Vec<serde_json::Value>> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/tags?per_page=100&page={page}",);
    debug!("Retrieving {}", &url);
    let body: String = add_auth(get_proxy_req()?.get(&url)).call()?.into_string()?;
    let json: serde_json::Value =
        serde_json::from_str(&body).context("Failed to parse github tags api")?;
    Ok(json
        .as_array()
        .context("No entries in github tags api?")?
        .clone())
}

fn could_be_a_sha1(input: &str) -> bool {
    input.len() == 40 && input.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_parse_vcs() {
        let vcs = ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/main/1.15.4")
            .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: Some("main".to_string()),
                rev: Some("1.15.4".to_string())
            }
        );

        let vcs =
            ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes//1.15.4").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("1.15.4".to_string())
            }
        );

        let vcs =
            ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/master").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: Some("master".to_string()),
                rev: None
            }
        );
        let vcs =
            ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/f34751b88bd07d7f44f5cd3200fb4122bf916c7e").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("f34751b88bd07d7f44f5cd3200fb4122bf916c7e".to_string())
            }
        );
        let vcs = ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: None
            }
        );
        let vcs = ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/f34751b88bd07d7f44f5cd3200fb4122bf916c7e").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("f34751b88bd07d7f44f5cd3200fb4122bf916c7e".to_string())
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?ref=main&rev=1.15.4",
        )
        .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: Some("main".to_string()),
                rev: Some("1.15.4".to_string())
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?rev=1.15.4",
        )
        .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("1.15.4".to_string())
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?ref=main",
        )
        .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: Some("main".to_string()),
                rev: None,
            }
        );
        let vcs =
            ParsedVCS::try_from("git+https://github.com/TyberiusPrime/anysnake2_release_flakes")
                .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: None,
                rev: None,
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?ref=main&rev=1.15.4&branch=shu",
        );
        assert!(vcs.is_err());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn vcs_to_string() {
        assert_eq!(
            "github:TyberiusPrime/anysnake2_release_flakes/main/1.15.4",
            TofuVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: "main".to_string(),
                rev: "1.15.4".to_string(),
            }
            .to_string()
        );
        assert_eq!(
            "github:TyberiusPrime/anysnake2_release_flakes/1.15.4",
            TofuVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: "main".to_string(),
                rev: "1.15.4".to_string(),
            }
            .to_nix_string()
        );

        let vcs =
            ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes//1.15.4").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("1.15.4".to_string())
            }
        );

        let vcs =
            ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/master").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: Some("master".to_string()),
                rev: None
            }
        );
        let vcs =
            ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/f34751b88bd07d7f44f5cd3200fb4122bf916c7e").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("f34751b88bd07d7f44f5cd3200fb4122bf916c7e".to_string())
            }
        );
        let vcs = ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: None
            }
        );
        let vcs = ParsedVCS::try_from("github:TyberiusPrime/anysnake2_release_flakes/f34751b88bd07d7f44f5cd3200fb4122bf916c7e").unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::GitHub {
                owner: "TyberiusPrime".to_string(),
                repo: "anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("f34751b88bd07d7f44f5cd3200fb4122bf916c7e".to_string())
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?ref=main&rev=1.15.4",
        )
        .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: Some("main".to_string()),
                rev: Some("1.15.4".to_string())
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?rev=1.15.4",
        )
        .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: None,
                rev: Some("1.15.4".to_string())
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?ref=main",
        )
        .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: Some("main".to_string()),
                rev: None,
            }
        );
        let vcs =
            ParsedVCS::try_from("git+https://github.com/TyberiusPrime/anysnake2_release_flakes")
                .unwrap();
        assert_eq!(
            vcs,
            ParsedVCS::Git {
                url: "https://github.com/TyberiusPrime/anysnake2_release_flakes".to_string(),
                branch: None,
                rev: None,
            }
        );
        let vcs = ParsedVCS::try_from(
            "git+https://github.com/TyberiusPrime/anysnake2_release_flakes?ref=main&rev=1.15.4&branch=shu",
        );
        assert!(vcs.is_err());
    }
}
