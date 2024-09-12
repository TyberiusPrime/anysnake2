use crate::config;
use anyhow::{anyhow, bail, Context, Result};
use ex::fs;
use itertools::Itertools;
#[allow(unused_imports)]
use log::{debug, error, info, trace};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::vcs;
use anysnake2::{run_without_ctrl_c, safe_python_package_name};

/// captures everything we need to know about an 'input' to our flake.
struct InputFlake {
    name: String,
    url: String,
    dir: Option<String>,
    follows: Vec<String>,
    is_flake: bool,
}

impl InputFlake {
    fn new(name: &str, url: &vcs::TofuVCS, dir: Option<String>, follows: &[&str]) -> Self {
        InputFlake {
            name: name.to_string(),
            url: url.to_nix_string(), // different  from to_string. To_string is what we need in
            dir,
            // anynsake2.toml, to_nix_string is what nix needs to see
            follows: follows.iter().map(ToString::to_string).collect(),
            is_flake: true,
        }
    }
}

struct Filenames {
    flake_filename: PathBuf,
    poetry_lock: PathBuf,
    pyproject_toml: PathBuf,
}

fn get_filenames(flake_dir: impl AsRef<Path>, use_generated_file_instead: bool) -> Filenames {
    if use_generated_file_instead {
        Filenames {
            flake_filename: flake_dir.as_ref().join("flake.temp.nix"),
            poetry_lock: flake_dir.as_ref().join("poetry_temp").join("poetry.lock"),
            pyproject_toml: flake_dir
                .as_ref()
                .join("poetry_temp")
                .join("pyproject.toml"),
        }
    } else {
        Filenames {
            flake_filename: flake_dir.as_ref().join("flake.nix"),
            poetry_lock: flake_dir.as_ref().join("poetry").join("poetry.lock"),
            pyproject_toml: flake_dir.as_ref().join("poetry").join("pyproject.toml"),
        }
    }
}

pub fn write_flake(
    flake_dir: impl AsRef<Path>,
    parsed_config: &mut config::TofuConfigToml,
    use_generated_file_instead: bool, // which is set if do_not_modify_flake is in effect.
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
) -> Result<bool> {
    let template = std::include_str!("nix/flake_template.nix");
    let flake_dir: &Path = flake_dir.as_ref();

    let filenames = get_filenames(flake_dir, use_generated_file_instead);
    let flake_filename = filenames.flake_filename;
    let old_flake_contents =
        fs::read_to_string(&flake_filename).unwrap_or_else(|_| String::default());
    //let old_poetry_lock = fs::read_to_string(&poetry_lock).unwrap_or_else(|_| "".to_string());

    let mut flake_contents: String = template.to_string();

    //various 'collectors'
    let mut inputs: Vec<InputFlake> = Vec::new();
    let mut definitions: BTreeMap<String, String> = BTreeMap::new();
    let mut overlays: Vec<String> = Vec::new();
    let rust_extensions: Vec<String> = Vec::new();
    let mut nixpkgs_pkgs = BTreeSet::new();
    let mut git_tracked_files = Vec::new();
    //let mut nix_pkg_overlays = Vec::new();

    // we always need the flake utils.
    inputs.push(InputFlake::new(
        "flake-utils",
        &parsed_config.flake_util,
        None,
        &[],
    ));

    // and nixpkgs is non optional as well.

    inputs.push(InputFlake::new(
        "nixpkgs",
        &parsed_config.nixpkgs.url,
        None,
        &[],
    ));
    nixpkgs_pkgs.extend(parsed_config.nixpkgs.packages.clone());
    nixpkgs_pkgs.insert("cacert".to_string()); //so we have SSL certs inside
                                               //
                                               ////todo: does rust even need to be a special case?
    add_rust(
        parsed_config,
        &mut inputs,
        &mut definitions,
        &mut overlays,
        &mut nixpkgs_pkgs,
        rust_extensions,
    );

    add_flakes(parsed_config, &mut inputs, &mut nixpkgs_pkgs);

    add_r(
        parsed_config,
        &mut inputs,
        &mut definitions,
        &mut overlays,
        &mut nixpkgs_pkgs,
    );

    let python_locks_changed = add_python(
        parsed_config,
        &mut inputs,
        &mut definitions,
        &mut nixpkgs_pkgs,
        &mut git_tracked_files,
        &filenames.pyproject_toml,
        &filenames.poetry_lock,
        flake_dir,
        in_non_spec_but_cached_values,
        out_non_spec_but_cached_values,
    )?;

    /* flake_contents = match &parsed_config.flakes {
        Some(flakes) =>
        None => flake_contents.replace("%FURTHER_FLAKE_PACKAGES%", ""),
    }; */
    /* let dev_shell_inputs = match &parsed_config.dev_shell.inputs {
        Some(dvi) => dvi.join(" "),
        None => "".to_string(),
    };
    flake_contents = flake_contents.replace("#%DEVSHELL_INPUTS%", &dev_shell_inputs);
    */

    /*
    let mut jupyter_kernels = String::new();
    let jupyter_included = python_packages.iter().any(|(k, _)| k == "jupyter");
    if let Some(r) = &parsed_config.r {
        if r.ecosystem_tag.is_some() {
            bail!("[R]ecosystem_tag is no longer in use. We're using nixR now and you need to specify a 'date' instead");
        }
        // install R kernel
        if r.packages.contains(&"IRkernel".to_string()) && jupyter_included {
            jupyter_kernels.push_str(
                "
            mkdir $out/rootfs/usr/share/jupyter/kernels/R
            cp $out/rootfs/R_libs/IRkernel/kernelspec/ * $out/rootfs/usr/share/jupyter/kernels/R -r
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
            cp $out/rootfs/usr/share/jupyter/kernels_/ * $out/rootfs/usr/share/jupyter/kernels -r
            unlink $out/rootfs/usr/share/jupyter/kernels_
            "
        .to_string()
            + &jupyter_kernels;
    }

    flake_contents = flake_contents.replace("#%INSTALL_JUPYTER_KERNELS%", &jupyter_kernels);
    */

    definitions.insert(
        "overlays".to_string(),
        format!(
            "[{}]",
            overlays.iter().map(|ov| format!("({ov})")).join(" ")
        ),
    );

    flake_contents = insert_nixpkgs_pkgs(&flake_contents, &nixpkgs_pkgs);
    flake_contents = insert_allow_unfree(
        &flake_contents,
        parsed_config.nixpkgs.allow_unfree,
        &parsed_config.nixpkgs.permitted_insecure_packages,
    );
    flake_contents = flake_contents
        .replace("#%INPUT_DEFS%", &format_input_defs(&inputs))
        .replace("#%INPUTS%", &format_inputs_for_output_arguments(&inputs))
        .replace("#%DEFINITIONS%#", &format_definitions(&definitions));

    // pretty print the generated flake
    flake_contents = nix_format(&flake_contents, flake_dir)?;

    /* if !overlays.is_empty() {
        flake_contents = flake_contents.replace(
            "\"%OVERLAY_AND_PACKAGES%\"",
            &("[".to_string() + &overlays.join(" ") + "]"),
        );
    } else {
        flake_contents = flake_contents.replace("\"%OVERLAY_AND_PACKAGES%\"", "[]");
    } */

    //print!("{}", flake_contents);
    create_flake_git(flake_dir, &mut git_tracked_files)?;
    /*
    if parsed_config.python.is_some() {
        gitargs.push("poetry/pyproject.toml");
        gitargs.push("poetry/poetry.lock");
    } */

    let mut res = write_flake_contents(
        &old_flake_contents,
        &flake_contents,
        use_generated_file_instead,
        &flake_filename,
        flake_dir,
    )?;
    res = res | python_locks_changed;

    run_git_add(&git_tracked_files, flake_dir)?;
    run_git_commit(flake_dir)?; //after nix 2.23 we will need to commit the flake, possibly. At
                                //least if we wanted to reference it from another flake

    Ok(res)
}

/// format the list of input flakes for the inputs = {} section of a flake.nix
fn format_input_defs(inputs: &[InputFlake]) -> String {
    let mut out = String::default();
    for fl in inputs {
        let v_follows: Vec<String> = fl
            .follows
            .iter()
            .map(|x| format!("        inputs.{}.follows = \"{}\";", &x, &x))
            .collect();
        let str_follows = v_follows.join("\n");
        let url = if fl.url.starts_with("github") && fl.url.matches('/').count() == 3 {
            //has a branch - but we define a revision, and you can't have both for some reason
            let mut iter = fl.url.rsplitn(2, '/');
            iter.next(); // eat the branch
            iter.collect()
        } else {
            fl.url.to_string()
        };
        let url = match &fl.dir {
            None => url,
            Some(dir) => format!("{url}?dir={dir}"),
        };
        out.push_str(&format!(
            "
    {} = {{
        url = \"{}\";
{}
{}
    }};",
            fl.name,
            url,
            &str_follows,
            if fl.is_flake { "" } else { "flake = false;" }
        ));
    }
    out
}

/// format the list of input flakes for the outputs {self, <arguments>}
fn format_inputs_for_output_arguments(inputs: &[InputFlake]) -> String {
    inputs.iter().map(|i| &i.name[..]).join(",\n    ")
}

fn format_definitions(definitions: &BTreeMap<String, String>) -> String {
    let mut res = String::default();
    for (k, v) in definitions {
        res.push_str(&format!("     {k} = {v};\n"));
    }
    res.trim().to_string()
}

fn insert_nixpkgs_pkgs(flake_contents: &str, nixpkgs_pkgs: &BTreeSet<String>) -> String {
    let str_nixpkgs_pkgs: String = nixpkgs_pkgs
        .iter()
        .map(|x| format!("${{{x}}}"))
        .collect::<Vec<String>>()
        .join("\n");
    flake_contents.replace("#%NIXPKGS_PACKAGES%#", &str_nixpkgs_pkgs)
}

fn insert_allow_unfree(
    flake_contents: &str,
    allow_unfree: bool,
    permitted_insecure_packages: &Option<Vec<String>>,
) -> String {
    let flake_contents = flake_contents.replace(
        "\"%ALLOW_UNFREE%\"",
        if allow_unfree { "true" } else { "false" },
    );
    flake_contents.replace(
        "\"%PERMITTED_INSECURE_PACKAGES%\"",
        &(permitted_insecure_packages
            .as_ref()
            .map_or_else(|| String::new(), |x| x.join(" "))),
    )
}

/// prepare what we put into pyproject.toml
#[allow(clippy::too_many_lines)]
fn prep_packages_for_pyproject_toml(
    input: &mut HashMap<String, config::TofuPythonPackageDefinition>,
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
    pyproject_toml_path: &Path,
) -> Result<toml::Table> {
    let mut result = toml::Table::new();
    for (name, spec) in input {
        match &spec.source {
            config::TofuPythonPackageSource::VersionConstraint(version_constraint) => {
                if version_constraint.contains("==")
                    || version_constraint.contains('>')
                    || version_constraint.contains('<')
                    || version_constraint.contains('!')
                {
                    result.insert(
                        name.to_string(),
                        toml::Value::String(version_constraint.to_string()),
                    );
                } else if version_constraint.contains('=') {
                    result.insert(
                        name.to_string(),
                        toml::Value::String(format!("={version_constraint}")),
                    );
                } else if version_constraint.is_empty() {
                    result.insert(name.to_string(), toml::Value::String("*".to_string()));
                } else {
                    result.insert(
                        name.to_string(),
                        toml::Value::String(version_constraint.to_string()),
                    );
                }
            }
            config::TofuPythonPackageSource::PyPi { version } => {
                result.insert(
                    name.to_string(),
                    toml::Value::String(format!("=={version}")),
                );
            }
            config::TofuPythonPackageSource::Url(url) => {
                let mut out_map: toml::Table = toml::Table::new();
                out_map.insert("url".to_string(), toml::Value::String(url.to_string()));
                result.insert(name.to_string(), toml::Value::Table(out_map));
            }
            config::TofuPythonPackageSource::Vcs(vcs) => match vcs {
                vcs::TofuVCS::GitHub {
                    owner,
                    repo,
                    branch: _,
                    rev,
                } => {
                    {
                        // poetry does git, but if it's not allowed to create virtual envs,
                        // it wants to clone into the python folder (!)
                        // https://github.com/python-poetry/poetry/issues/9470
                        // so we do the same thing as for mercurial, clone into the nix store,
                        // add a nix fetchgit dependency, and rewrite it to work inside poetry2nix
                        let (path, sha256) = clone_to_nix_store(
                            &format!("github:{owner}/{repo}"),
                            rev,
                            "git",
                            prefetch_github_store_path,
                            in_non_spec_but_cached_values,
                            out_non_spec_but_cached_values,
                        )?;
                        let writeable_path = copy_for_poetry(
                            &path,
                            name,
                            &sha256,
                            pyproject_toml_path,
                            &spec.pre_poetry_patch,
                        )?;
                        let mut out_map = toml::Table::new();
                        out_map.insert("path".to_string(), writeable_path.into());
                        let src = format!(
                            "(
                            pkgs.fetchFromGitHub {{
                                    owner = \"{owner}\";
                                    repo = \"{repo}\";
                                    rev = \"{rev}\";
                                    hash = \"{sha256}\";
                            }})",
                        );
                        spec.poetry2nix.insert("src".to_string(), src.into());
                        result.insert(name.to_string(), toml::Value::Table(out_map));
                    }
                }
                vcs::TofuVCS::Git {
                    url,
                    branch: _,
                    rev,
                } => {
                    // poetry does git, but if it's not allowed to create virtual envs,
                    // it wants to clone into the python folder (!)
                    // https://github.com/python-poetry/poetry/issues/9470
                    // so we do the same thing as for mercurial, clone into the nix store,
                    // add a nix fetchgit dependency, and rewrite it to work inside poetry2nix
                    let (path, sha256) = clone_to_nix_store(
                        url,
                        rev,
                        "git",
                        prefetch_git_store_path,
                        in_non_spec_but_cached_values,
                        out_non_spec_but_cached_values,
                    )?;
                    let writeable_path = copy_for_poetry(
                        &path,
                        name,
                        &sha256,
                        pyproject_toml_path,
                        &spec.pre_poetry_patch,
                    )?;
                    let mut out_map = toml::Table::new();
                    out_map.insert("path".to_string(), writeable_path.into());
                    let src = format!(
                        "(
                            pkgs.fetchgit {{
                                    url = \"{url}\";
                                    rev = \"{rev}\";
                                    hash = \"{sha256}\";
                            }})",
                    );
                    spec.poetry2nix.insert("src".to_string(), src.into());
                    result.insert(name.to_string(), toml::Value::Table(out_map));
                }
                vcs::TofuVCS::Mercurial { url, rev } => {
                    // poetry does not do mercurial
                    // but nix does.
                    // and poetry does paths.
                    // so we can put the nix store path into poetry.toml
                    // later rewrite it to work inside poetry2nix (which assumes relativ paths)
                    // and add the nix fetchhg to the python packages src.
                    let (path, sha256) = clone_to_nix_store(
                        url,
                        rev,
                        "mercurial",
                        prefetch_hg_store_path,
                        in_non_spec_but_cached_values,
                        out_non_spec_but_cached_values,
                    )?;
                    let mut out_map = toml::Table::new();
                    out_map.insert("path".to_string(), path.into());
                    let src = format!(
                        "(
                            pkgs.fetchhg {{
                                    url = \"{url}\";
                                    rev = \"{rev}\";
                                    hash = \"{sha256}\";
                            }})",
                    );
                    spec.poetry2nix.insert("src".to_string(), src.into());
                    result.insert(name.to_string(), toml::Value::Table(out_map));
                }
            },
        }
    }
    Ok(result)
}

/// poetry needs *writable* clones of the repos,
/// because it needs to build egg-infos that write into the checkout
fn copy_for_poetry(
    path: &str,
    name: &str,
    sha256: &str,
    pyproject_toml_path: &Path,
    pre_poetry_patch: &Option<String>,
) -> Result<String> {
    let pre_poetry_patch_sha = pre_poetry_patch
        .as_ref()
        .map(|x| sha256::digest(x))
        .unwrap_or_else(|| "None".to_string());

    let target_path = pyproject_toml_path
        .parent()
        .unwrap()
        .join(name)
        .join(format!("{sha256}-{pre_poetry_patch_sha}"));
    //copy the full path, using cp...
    if !target_path.exists() {
        ex::fs::create_dir_all(target_path.parent().unwrap())?;
        info!("Copying {} to {}", path, target_path.to_string_lossy());
        let mut cmd = Command::new("cp");
        cmd.args(["-r", path, &target_path.to_string_lossy()]);
        debug!("cmd: {:?}", cmd);
        cmd.status()?;
        // now chmod it to be writable
        Command::new("chmod")
            .args(["-R", "ug+w", &target_path.to_string_lossy()])
            .status()?;
        info!("Executing prePoetryPatch for {}", name);
        if let Some(pre_poetry_patch) = pre_poetry_patch {
            let mut cmd = Command::new("bash")
                .current_dir(&target_path)
                .stdin(Stdio::piped())
                .spawn()?;
            {
                let stdin = cmd.stdin.as_mut().unwrap();
                stdin.write_all(format!("set -xeou pipefail\n{pre_poetry_patch}").as_bytes())?;
            }
            let output = cmd.wait().context("prePoetryPatch failed")?;
            if output.success() {
                info!("prePoetryPatch succeeded");
            } else {
                bail!("prePoetryPatch failed");
            }
        }
    }
    Ok(target_path.canonicalize()?.to_string_lossy().to_string())
}

/// clone a repo to the nix store, return path and sha256 for the relevant fetch method.
/// also caches the value in the 'non-spec-but-cached' region of `.anysnake2_flake`
fn clone_to_nix_store(
    url: &str,
    rev: &str,
    prefix: &str,
    prefetch_func: impl Fn(&str, &str) -> Result<PrefetchResult>,
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
) -> Result<(String, String)> {
    let path_key = format!("{prefix}/{url}/{rev}/path");
    let hash_key = format!("{prefix}/{url}/{rev}/sha256");
    let path = in_non_spec_but_cached_values.get(&path_key);
    let sha256 = in_non_spec_but_cached_values.get(&hash_key);
    #[allow(clippy::pedantic)]
    let (path, sha256) = match (path, sha256) {
        (Some(path), Some(sha256)) => (path.to_string(), sha256.to_string()),
        _ => {
            let path_and_hash = prefetch_func(url, rev)?;
            (path_and_hash.path, path_and_hash.sha256)
        }
    };
    out_non_spec_but_cached_values.insert(path_key, path.clone());
    out_non_spec_but_cached_values.insert(hash_key, sha256.clone());
    Ok((path, sha256))
}

fn get_basic_auth_header(user: &str, pass: &str) -> String {
    use base64::Engine;
    let usrpw = String::from(user) + ":" + pass;
    String::from("Basic ") + &base64::engine::general_purpose::STANDARD.encode(usrpw.as_bytes())
}

pub fn add_auth(mut request: ureq::Request) -> ureq::Request {
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

fn nix_format(input: &str, flake_dir: impl AsRef<Path>) -> Result<String> {
    let full_url = format!("{}#nixfmt", anysnake2::get_outside_nixpkgs_url().unwrap());
    // debug!("registering nixfmt with {}", &full_url);
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
        let input_with_line_nos = input
            .lines()
            .enumerate()
            .map(|(i, x)| format!("{i:4}\t{x}"))
            .collect::<Vec<String>>()
            .join("\n");
        Err(anyhow!(
            "nix fmt error return{}\n{}",
            out.status.code().unwrap(),
            input_with_line_nos
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn ancient_poetry(
    ancient_poetry: &vcs::TofuVCS,
    nixpkgs: &config::TofuNixpkgs,
    python_packages: &HashMap<String, config::TofuPythonPackageDefinition>,
    python_package_definitions: &toml::Table,
    pyproject_toml_path: &Path,
    poetry_lock_path: &Path,
    python_version: &str,
    python_major_minor: &str,
    date: chrono::NaiveDate,
) -> Result<()> {
    //let mut pyproject_toml_contents = toml::Table::new();
    //pyproject_toml_contents["tool.poetry"] = toml::Value::Table(toml::Table::new());
    let mut pyproject_toml_contents: toml::Table = r#"
[tool.poetry]
name = "anysnake2_to_ancient_poetry"
version = "0.1.0"
description = ""
authors = ["Nemo"]

[build-system]
requires = ["poetry-core"]
build-backend = "poetry.core.masonry.api"


"#
    .parse()
    .unwrap();
    let mut dependencies = toml::Table::new();
    //[tool.poetry.dependencies]
    dependencies.insert(
        "python".to_string(),
        toml::Value::String(format!("~={python_version}.0")),
    );
    for (name, version_constraint) in python_package_definitions {
        dependencies.insert(name.to_string(), version_constraint.clone());
    }
    pyproject_toml_contents["tool"]["poetry"]
        .as_table_mut()
        .unwrap()
        .insert("dependencies".to_string(), toml::Value::Table(dependencies));
    ex::fs::create_dir_all(pyproject_toml_path.parent().unwrap())?;
    let pyproject_contents = pyproject_toml_contents.to_string();
    ex::fs::write(pyproject_toml_path, &pyproject_contents)?;
    let pyproject_toml_hash = sha256::digest(pyproject_contents);

    let last_hash = ex::fs::read_to_string(pyproject_toml_path.with_extension("sha256"))
        .unwrap_or(String::new())
        .trim()
        .to_string();
    if (pyproject_toml_hash != last_hash)
        || !poetry_lock_path.exists()
        || poetry_lock_path.metadata()?.len() == 0
    {
        //todo make configurable
        let full_url = ancient_poetry.to_nix_string();
        let str_date = date.format("%Y-%m-%d").to_string();

        let exclusion_list = python_packages
            .iter()
            .filter_map(|(name, spec)| match &spec.source {
                config::TofuPythonPackageSource::PyPi { version } => {
                    Some(format!("{name}={version}"))
                }
                _ => None,
            })
            .join(" ");

        let mut full_args = vec![
            "shell".into(),
            format!("{}#poetry", anysnake2::get_outside_nixpkgs_url().unwrap()),
            format!("{}#{}", nixpkgs.url.to_nix_string(), python_major_minor),
            full_url,
            "-c".into(),
            "ancient-poetry".into(),
            "-t".into(),
            str_date,
            "-p".into(),
            pyproject_toml_path.to_string_lossy().to_string(),
            "-o".into(),
            poetry_lock_path.to_string_lossy().to_string(),
        ];
        if !exclusion_list.is_empty() {
            full_args.push("-e".into());
            full_args.push(exclusion_list);
        }
        debug!(
            "running ancient-poetry: nix {}",
            full_args.iter().map(|x| format!("\"{x}\"")).join(" ")
        );
        let out = Command::new("nix")
            .args(full_args)
            .current_dir(".")
            //.stdin(Stdio::piped())
            //.stdout(Stdio::piped())
            .status()?;
        if out.success() {
            //write it to poetry.lock
            //ex::fs::write(poetry_lock_path, stdout)?;
            ex::fs::write(
                pyproject_toml_path.with_extension("sha256"),
                pyproject_toml_hash,
            )?;
            Ok(())
        } else {
            Err(anyhow!(
                "ancient-poetry error returncode: {}\n",
                out.code().unwrap(),
            ))
        }
    } else {
        debug!("Skipping call to ancient poetry - pyproject.toml matches last run");
        Ok(())
    }
}

fn create_flake_git(flake_dir: &Path, tracked_files: &mut Vec<String>) -> Result<()> {
    let mut git_path = flake_dir.to_path_buf();
    git_path.push(".git");
    if !git_path.exists() {
        let output = Command::new("git")
            .args(["init"])
            .current_dir(flake_dir)
            .output()
            .context(format!("Failed create git repo in {flake_dir:?}"))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = format!(
                "Failed to init git repo in  {flake_dir:?}.\n Stdout {stdout:?}\nStderr: {stderr:?}",
            );
            bail!(msg);
        }
    }

    tracked_files.extend(
        ["flake.nix", "functions.nix", ".gitignore"]
            .iter()
            .map(ToString::to_string),
    );
    if flake_dir.join("flake.lock").exists() {
        tracked_files.push("flake.lock".to_string());
    }
    fs::write(
        flake_dir.join(".gitignore"),
        "result
    run_scripts/
    .*.json
    .gc_roots
    ",
    )?;

    Ok(())
}

fn write_flake_contents(
    old_flake_contents: &str,
    flake_contents: &str,
    use_generated_file_instead: bool,
    flake_filename: &Path,
    flake_dir: &Path,
) -> Result<bool> {
    let res = if use_generated_file_instead {
        if old_flake_contents != flake_contents {
            fs::write(flake_filename, flake_contents)?;
        }
        Ok(true)
    } else if old_flake_contents != flake_contents {
        fs::write(flake_filename, flake_contents)
            .with_context(|| format!("failed writing {:?}", &flake_filename))?;

        Ok(true)
    } else {
        debug!("flake unchanged");
        Ok(false)
    };
    fs::write(
        flake_dir.join("functions.nix"),
        include_str!("nix/functions.nix"),
    )?;
    res
}

fn run_git_add(tracked_files: &[String], flake_dir: &Path) -> Result<()> {
    let output = run_without_ctrl_c(|| {
        Command::new("git")
            .arg("add")
            .args(tracked_files)
            .current_dir(flake_dir)
            .output()
            .context("Failed git add")
    })?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!("Failed git add flake.nix. \n Stdout {stdout:?}\nStderr: {stderr:?}",);
        bail!(msg);
    }
    Ok(())
}
fn run_git_commit(flake_dir: &Path) -> Result<()> {
    let output = run_without_ctrl_c(|| {
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("automatic")
            .current_dir(flake_dir)
            .output()
            .context("Failed git commit")
    })?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stdout.contains("no changes added") {
            let msg =
                format!("Failed git add flake.nix. \n Stdout {stdout:?}\nStderr: {stderr:?}",);
            bail!(msg);
        }
    }
    Ok(())
}
fn add_rust(
    parsed_config: &config::TofuConfigToml,
    inputs: &mut Vec<InputFlake>,
    definitions: &mut BTreeMap<String, String>,
    overlays: &mut Vec<String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
    rust_extensions: Vec<String>,
) {
    if let Some(rust) = &parsed_config.rust {
        if let Some(rust_ver) = &rust.version {
            nixpkgs_pkgs.insert("stdenv.cc".to_string()); // needed to actually build something with rust
            let mut out_rust_extensions = vec!["rustfmt".to_string(), "clippy".to_string()];
            out_rust_extensions.extend(rust_extensions);

            inputs.push(InputFlake::new(
                "rust-overlay",
                &rust.url,
                None,
                &["nixpkgs", "flake-utils"],
            ));
            overlays.push("import rust-overlay".to_string());
            let str_rust_extensions: Vec<String> = out_rust_extensions
                .into_iter()
                .map(|x| format!("\"{x}\""))
                .collect();
            let str_rust_extensions: String = str_rust_extensions.join(" ");

            definitions.insert(
                "rust".to_string(),
                format!(
            "pkgs.rust-bin.stable.\"{rust_ver}\".minimal.override {{ extensions = [ {str_rust_extensions}]; }}",
        ),
            );
            nixpkgs_pkgs.insert("rust".to_string());
        };
    }
}

fn add_flakes(
    parsed_config: &config::TofuConfigToml,
    inputs: &mut Vec<InputFlake>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
) {
    {
        let flakes = &parsed_config.flakes;
        let mut names: Vec<&String> = flakes.keys().collect();
        names.sort();
        for name in names {
            let flake = flakes.get(name).unwrap();
            let rev_follows: Vec<&str> = match &flake.follows {
                Some(f) => f.iter().map(|x| &x[..]).collect(),
                None => Vec::new(),
            };
            inputs.push(InputFlake::new(
                name,
                &flake.url, // at this point we must have a rev,
                flake.dir.clone(),
                &rev_follows[..],
            ));
            if flake.packages.is_empty() {
                nixpkgs_pkgs.insert(format!("{}.{}", name, "defaultPackage.x86_64-linux"));
            } else {
                for pkg in &flake.packages {
                    nixpkgs_pkgs.insert(format!("{name}.{pkg}"));
                }
            }
        }
    }
}

fn add_r(
    parsed_config: &config::TofuConfigToml,
    inputs: &mut Vec<InputFlake>,
    definitions: &mut BTreeMap<String, String>,
    overlays: &mut Vec<String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
) {
    fn attrset_from_hashmap(attrset: &HashMap<String, String>) -> String {
        let mut out = String::new();
        for (pkg_name, override_nix_func) in attrset {
            out.push_str(&format!("\"{pkg_name}\" = ({override_nix_func});"));
        }
        out
    }

    if let Some(r_config) = &parsed_config.r {
        inputs.push(InputFlake::new("nixR", &r_config.url, None, &[]));

        let r_override_args = r_config
            .override_attrs
            .as_ref()
            .map_or(String::new(), attrset_from_hashmap);
        let r_dependency_overrides = r_config
            .dependency_overrides
            .as_ref()
            .map_or(String::new(), attrset_from_hashmap);
        let r_additional_packages = r_config
            .additional_packages
            .as_ref()
            .map_or(String::new(), attrset_from_hashmap);

        let mut r_pkg_list: Vec<String> =
            r_config.packages.iter().map(ToString::to_string).collect();
        if let Some(additional_packages) = &r_config.additional_packages {
            for pkg_ver in additional_packages.keys() {
                let (pkg, _ver) = pkg_ver
                    .split_once('_')
                    .expect("R.additional_packages key did not conform to 'name_version' schema");
                r_pkg_list.push(pkg.to_string());
            }
        }
        //remove duplicates
        r_pkg_list.sort();
        r_pkg_list.dedup();

        let nix_nix_pkgs = if r_config.use_inside_nix_pkgs.unwrap_or(true) {
            "pkgs"
        } else {
            "null"
        };

        let r_packages = format!(
            "
                nixR.R_by_date {{
                    date = \"{}\" ;
                    r_pkg_names = [{}];
                    nix_pkgs_pkgs = {nix_nix_pkgs};
                    packageOverrideAttrs = {{ {} }};
                    r_dependency_overrides = {{ {} }};
                    additional_packages = {{ {} }};
                }}
                ",
            &r_config.date,
            r_pkg_list.iter().map(|x| format!("\"{x}\"")).join(" "),
            r_override_args,
            r_dependency_overrides,
            r_additional_packages
        );
        definitions.insert("R_tracked".to_string(), r_packages);
        overlays.push(
            "(final: prev: {
                R = R_tracked // {meta = { platforms=prev.R.meta.platforms;};};
                rPackages = R_tracked.rPackages;
                }) "
            .to_string(),
        );

        nixpkgs_pkgs.insert("R".to_string()); // that's the overlayed R.
    }
}

fn format_poetry_build_input_overrides(
    python_packages: &HashMap<String, config::TofuPythonPackageDefinition>,
) -> Result<Vec<String>> {
    let mut poetry_overide_entries = Vec::new();
    for (name, spec) in python_packages {
        let mut override_python_attrs = HashMap::new();
        let mut overrides = HashMap::new();
        for kk in &["buildInputs", "propagatedBuildInputs", "nativeBuildInputs"] {
            if let Some(build_inputs) = spec.poetry2nix.get(*kk) {
                let str_build_inputs = build_inputs
                    .as_array()
                    .with_context(|| {
                        format!("{kk} was not a list of strings package definition for {name}",)
                    })?
                    .iter()
                    .map(|v| {
                        Ok({
                            let v = v.as_str().with_context(|| {
                                format!(
                                    "{kk} was not a list of strings package definition for {name}",
                                )
                            })?;
                            if v.starts_with('(') {
                                v.to_string()
                            } else {
                                format!("prev.{v}")
                            }
                        })
                    })
                    .collect::<Result<Vec<String>>>()?
                    .join(" ");
                override_python_attrs
                    .insert(*kk, format!("(old.{kk} or []) ++ [{str_build_inputs}]"));
            }
        }

        if let Some(src) = spec.poetry2nix.get("src") {
            let src = src
                .as_str()
                .with_context(|| format!("src was not a string with nix code for {name}",))?;
            override_python_attrs.insert("src", src.to_string());
        }

        /* if let Some(post_patch) = spec.poetry2nix.get("postPatch") {
            let post_patch = post_patch
                .as_str()
                .with_context(|| format!("postPatch was not a string with bash code for {name}",))?;
            override_python_attrs.insert("postPatch", format!("''{post_patch}''"));
        } */

        if let Some(envs) = spec.poetry2nix.get("env") {
            let envs = envs
                .as_table()
                .with_context(|| format!("envs was not a table {name}",))?;
            for (k, v) in envs {
                let v = v
                    .as_str()
                    .with_context(|| format!("envs entry was not a string for {name}"))?;
                override_python_attrs.insert(k, format!("''{v}''"));
            }
        }
        if let Some(further) = spec.poetry2nix.get("overridePythonAttrs") {
            let further = further
                .as_table()
                .with_context(|| format!("envs was not a table {name}",))?;
            for (k, v) in further {
                let v = v.as_str().with_context(|| {
                    format!("envs entry was not a string (with nix code!) for {name}")
                })?;
                override_python_attrs.insert(k, v.to_string());
            }
        }

        if let Some(prefer_wheel) = spec.poetry2nix.get("preferWheel") {
            let prefer_wheel = prefer_wheel
                .as_bool()
                .with_context(|| format!("preferWheel was not a boolean for {name}",))?;
            overrides.insert("preferWheel", format!("{prefer_wheel}"));
        }
        if !override_python_attrs.is_empty() || !overrides.is_empty() {
            let str_overrides = override_python_attrs
                .iter()
                .map(|(k, v)| format!("{k} = {v};"))
                .collect::<Vec<String>>()
                .join(" ");
            let safe_name = safe_python_package_name(name);
            let first_part = if overrides.is_empty() {
                format!("prev.{safe_name}")
            } else {
                let override_str = overrides
                    .iter()
                    .map(|(k, v)| format!("{k} = {v};"))
                    .collect::<Vec<String>>()
                    .join("\n");
                format!("(prev.{safe_name}.override {{{override_str}}})",)
            };
            poetry_overide_entries.push(format!(
                "{safe_name} = {first_part}.overridePythonAttrs (old: rec {{{str_overrides}}});"
            ));
        }
    }
    poetry_overide_entries.sort();
    Ok(poetry_overide_entries)
}

#[allow(clippy::too_many_arguments)]
fn add_python(
    parsed_config: &mut config::TofuConfigToml,
    inputs: &mut Vec<InputFlake>,
    definitions: &mut BTreeMap<String, String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
    git_tracked_files: &mut Vec<String>,
    pyproject_toml_path: &Path,
    poetry_lock_path: &Path,
    flake_dir: &Path,
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
) -> Result<bool> {
    //ex::fs::create_dir_all(poetry_lock.parent().unwrap())?;
    let mut changed = false;
    match &mut parsed_config.python {
        Some(python) => {
            let original_pyproject_toml =
                ex::fs::read_to_string(pyproject_toml_path).unwrap_or_else(|_| "".to_string());
            let original_poetry_lock =
                ex::fs::read_to_string(poetry_lock_path).unwrap_or_else(|_| "".to_string());

            if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                bail!(
                            format!("Python version must be x.y (not x.y.z, z is given by nixpkgs version). Was '{}'", &python.version));
            }
            let python_major_minor = format!("python{}", python.version.replace('.', ""));

            let mut out_python_packages = prep_packages_for_pyproject_toml(
                &mut python.packages,
                in_non_spec_but_cached_values,
                out_non_spec_but_cached_values,
                pyproject_toml_path,
            )?;
            if python.has_editable_packages() && !out_python_packages.contains_key("pip") {
                out_python_packages
                    .insert("pip".to_string(), toml::Value::String(">0".to_string()));
            }

            //out_python_packages.sort();
            let python_version = python.version.clone();
            let ecosystem_date = python.parsed_ecosystem_date()?;

            ancient_poetry(
                &parsed_config.ancient_poetry,
                &parsed_config.nixpkgs,
                &python.packages,
                &out_python_packages,
                pyproject_toml_path,
                poetry_lock_path,
                &python_version,
                &python_major_minor,
                ecosystem_date,
            )?;

            rewrite_poetry(flake_dir)?;

            let poetry_build_input_overrides =
                format_poetry_build_input_overrides(&python.packages)?;

            inputs.push(InputFlake::new(
                "poetry2nix",
                &parsed_config.poetry2nix.source,
                None,
                &[],
            ));

            definitions.insert(
                "_poetry2nix".to_string(),
                "if (builtins.hasAttr \"lib\" poetry2nix)
        then
          (
            poetry2nix.lib.mkPoetry2Nix {inherit pkgs;}
          )
        else poetry2nix.legacyPackages.${system}"
                    .to_string(), // that's support for older poetry2nix, but I don't think they
                                  // actually work.
            );
            definitions.insert(
                "mkPoetryEnv".to_string(),
                "_poetry2nix.mkPoetryEnv".to_string(),
            );
            definitions.insert(
                "defaultPoetryOverrides".to_string(),
                "_poetry2nix.defaultPoetryOverrides".to_string(),
            );
            //            definitions.insert("python_packages".to_string(), out_python_packages);
            definitions.insert(
                "python_version".to_string(),
                format!("pkgs.{python_major_minor}"),
            );

            definitions.insert(
                "poetry_overrides".to_string(),
                format!(
                    "(defaultPoetryOverrides.extend (final: prev: {{{}}}))",
                    poetry_build_input_overrides.join("\n"),
                ),
            );
            let prefer_wheels = parsed_config.poetry2nix.prefer_wheels;
            definitions.insert(
                "python_package".to_string(),
                format!(
                    "mkPoetryEnv {{
                                  projectDir = ./poetry_rewritten;
                                  python = python_version;
                                  preferWheels = {prefer_wheels};
                                  overrides = poetry_overrides;
                }}"
                ),
            );
            nixpkgs_pkgs.insert("python_package".to_string());

            //flake_contents
            //.replace("%PYTHON_MAJOR_MINOR%", &python_major_minor)
            /* .replace("%PYTHON_PACKAGES%", &out_python_packages)
            .replace("PYTHON_BUILD_PACKAGES", &out_python_build_packages)
            /* .replace(
                "PYTHON_ADDITIONAL_MKPYTHON_ARGUMENTS_FUNC",
                out_additional_mkpython_arguments_func,
            ) */
            .replace("%PYPI_DEPS_DB_REV%", &pypi_debs_db_rev) */
            git_tracked_files.push("poetry_rewritten/poetry.lock".to_string());
            git_tracked_files.push("poetry_rewritten/pyproject.toml".to_string());
            let new_pyproject_toml =
                ex::fs::read_to_string(pyproject_toml_path).unwrap_or_else(|_| "".to_string());
            let new_poetry_lock =
                ex::fs::read_to_string(poetry_lock_path).unwrap_or_else(|_| "".to_string());
            if new_pyproject_toml != original_pyproject_toml
                || new_poetry_lock != original_poetry_lock
            {
                changed = true;
            }
        }
        None => {
            if poetry_lock_path.exists() {
                ex::fs::remove_file(poetry_lock_path)?;
                changed = true;
            }
        }
    };

    Ok(changed)
}

struct PrefetchResult {
    path: String,
    sha256: String,
}

fn prefetch_hg_store_path(url: &str, rev: &str) -> Result<PrefetchResult> {
    let nix_prefetch_hg_url = format!(
        "{}#nix-prefetch-hg",
        anysnake2::get_outside_nixpkgs_url().unwrap()
    );
    let nix_prefetch_hg_url_args = vec![
        "shell",
        &nix_prefetch_hg_url,
        "-c",
        "nix-prefetch-hg",
        url,
        rev,
    ];
    let mut proc = Command::new("nix");
    proc.args(nix_prefetch_hg_url_args);
    debug!("running {proc:?}");
    let proc_res = proc.output().context("failed on nix-prefetch-hg")?;
    if !proc_res.status.success() {
        bail!("nix-prefetch-hg failed with code {}", proc_res.status);
    }

    let stdout = std::str::from_utf8(&proc_res.stdout)?.trim();
    let stderr = std::str::from_utf8(&proc_res.stderr)?.trim();
    let lines = stderr.split('\n'); // path is is in stderr.
    let path = lines
        .filter_map(|x| x.split_once("path is "))
        .map(|(_, x)| x)
        .next()
        .with_context(|| {
            format!("Could not find 'path is ' line in nix-prefetch-hg output. Output was {stdout}")
        })?
        .to_string();
    let hash = stdout.trim();
    let sha256 = crate::tofu::convert_hash_to_subresource_format(hash)?;

    Ok(PrefetchResult { path, sha256 })
}

fn prefetch_git_store_path(url: &str, rev: &str) -> Result<PrefetchResult> {
    let nix_prefetch_git_url = format!(
        "{}#nix-prefetch-git",
        anysnake2::get_outside_nixpkgs_url().unwrap()
    );
    let nix_prefetch_git_url_args = vec![
        "shell",
        &nix_prefetch_git_url,
        "-c",
        "nix-prefetch-git",
        url,
        rev,
    ];
    let mut proc = Command::new("nix");
    proc.args(nix_prefetch_git_url_args);
    debug!("running {proc:?}");
    let proc_res = proc.output().context("failed on nix-prefetch-git")?;
    if !proc_res.status.success() {
        bail!("nix-prefetch-git failed with code {}", proc_res.status);
    }

    let stdout = std::str::from_utf8(&proc_res.stdout)?.trim();
    let _stderr = std::str::from_utf8(&proc_res.stderr)?.trim();

    let structured: HashMap<String, serde_json::Value> =
        serde_json::from_str(stdout).context("nix-prefetch-git output failed json parsing")?;
    let old_format = structured
        .get("sha256")
        .context("No sha256 in nix-prefetch-git json output")?
        .as_str()
        .context("sha256 in nix-prefetch-git json was not a string")?;
    let sha256 = crate::tofu::convert_hash_to_subresource_format(old_format)?;
    let path = structured
        .get("path")
        .context("No path in nix-prefetch-git json output")?
        .as_str()
        .context("path in nix-prefetch-git json output was not a string")?
        .to_string();

    Ok(PrefetchResult { path, sha256 })
}
fn prefetch_github_store_path(url: &str, rev: &str) -> Result<PrefetchResult> {
    //every single one of the nix-prefetch-* fails me here
    //nix-prefetch-github: doesn't get you the store path
    //nix-prefetch doesn't actually realize the store path.
    //so let's do this ourselves...
    let (_, owner_repo) = url.split_once("github:").unwrap();
    let (owner, repo) = owner_repo.split_once('/').unwrap();

    let temp_dir = tempfile::TempDir::with_prefix("anysnake2_nix_prefetch_github")?;
    {
        // let td = PathBuf::from("temp"); // if you need to debug
        let td = temp_dir.path();
        let default_nix = td.join("default.nix");
        std::fs::write(
            &default_nix,
            format!(
                "
                {{ pkgs ? import <nixpkgs> {{}} }}:

                  pkgs.fetchFromGitHub {{
                    owner = \"{owner}\";
                    repo = \"{repo}\";
                    rev = \"{rev}\";
                    sha256 = pkgs.lib.fakeSha256;
                  }}
                "
            ),
        )
        .context("Could not write default.nix")?;
        let output = Command::new("nix-build")
            .args(["default.nix"])
            .current_dir(td)
            .output()
            .context("nix-build call failed")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let sha_re = regex::Regex::new(r"got:\s+(sha256-[^=]+)").unwrap();
        let hit = sha_re.captures(&stderr).with_context(||
            format!("nix-build failed with stdout: {stdout} stderr: {stderr}. Expected got: <sha256> line",
        ))?;
        let new_sha = hit.get(1).unwrap().as_str();
        std::fs::write(
            &default_nix,
            format!(
                "
                {{ pkgs ? import <nixpkgs> {{}} }}:

                  pkgs.fetchFromGitHub {{
                    owner = \"{owner}\";
                    repo = \"{repo}\";
                    rev = \"{rev}\";
                    sha256 = \"{new_sha}\";
                  }}
                "
            ),
        )
        .context("failed to write default.nix 2nd time")?;
        let output = Command::new("nix-build")
            .args(["default.nix"])
            .current_dir(td)
            .output()
            .context("nix-build call failed")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            bail!("nix-build for fetchFromGitHub failed: Stdout: {stdout}, stderr: {stderr}");
        }
        let result_path = td.join("result");
        let store_path = result_path
            .canonicalize()
            .context("failed to canonicalize nix store path")?;
        let path = store_path.to_string_lossy().to_string();
        Ok(PrefetchResult {
            path,
            sha256: new_sha.to_string(),
        })
    }
}

/// rewrite all /nix/store references in poetry.toml and lock into ../, and place in new folder
fn rewrite_poetry(flake_dir: &Path) -> Result<()> {
    ex::fs::create_dir_all(flake_dir.join("poetry_rewritten"))?;

    let filename = "pyproject.toml";
    let input_filename = flake_dir.join("poetry").join(filename);
    let output_filename = flake_dir.join("poetry_rewritten").join(filename);
    let raw = ex::fs::read_to_string(input_filename)?;
    let out = raw.replace("/nix/store/", "../");
    ex::fs::write(output_filename, out)?;

    let filename = "poetry.lock";
    let input_filename = flake_dir.join("poetry").join(filename);
    let output_filename = flake_dir.join("poetry_rewritten").join(filename);
    let raw = ex::fs::read_to_string(input_filename)?;
    let search_re = regex::Regex::new(r"(\.\./)+nix/store/").unwrap();
    let out = search_re.replace(&raw, "../../").to_string(); //todo: do it without the alloc
    ex::fs::write(output_filename, out)?;

    Ok(())
}
