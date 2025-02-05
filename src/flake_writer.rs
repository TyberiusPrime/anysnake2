use crate::config::{self, SafePythonName};
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
use anysnake2::run_without_ctrl_c;

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
    uv_lock: PathBuf,
    pyproject_toml: PathBuf,
}

fn get_filenames(flake_dir: impl AsRef<Path>, use_generated_file_instead: bool) -> Filenames {
    if use_generated_file_instead {
        Filenames {
            flake_filename: flake_dir.as_ref().join("flake.temp.nix"),
            uv_lock: flake_dir.as_ref().join("uv_temp").join("uv.lock"),
            pyproject_toml: flake_dir.as_ref().join("uv_temp").join("pyproject.toml"),
        }
    } else {
        Filenames {
            flake_filename: flake_dir.as_ref().join("flake.nix"),
            uv_lock: flake_dir.as_ref().join("uv").join("uv.lock"),
            pyproject_toml: flake_dir.as_ref().join("uv").join("pyproject.toml"),
        }
    }
}

pub struct WriteFlakeResult {
    pub flake_nix_changed: bool,
    pub python_lock_changed: bool,
}

pub fn write_flake(
    flake_dir: impl AsRef<Path>,
    parsed_config: &mut config::TofuConfigToml,
    use_generated_file_instead: bool, // which is set if do_not_modify_flake is in effect.
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
) -> Result<WriteFlakeResult> {
    let template = std::include_str!("nix/flake_template.nix");
    let flake_dir: &Path = flake_dir.as_ref();

    let filenames = get_filenames(flake_dir, use_generated_file_instead);
    let flake_filename = filenames.flake_filename;
    let old_flake_contents =
        fs::read_to_string(&flake_filename).unwrap_or_else(|_| String::default());

    let mut flake_contents: String = template.to_string();

    //various 'collectors'
    let mut inputs: Vec<InputFlake> = Vec::new();
    let mut definitions: BTreeMap<String, String> = BTreeMap::new();
    let mut overlays: Vec<String> = Vec::new();
    let mut rust_extensions: Vec<String> = Vec::new();
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
    add_jupyter_kernels(
        parsed_config,
        &mut definitions,
        &mut nixpkgs_pkgs,
        &mut rust_extensions,
    );

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
        &mut overlays,
        &filenames.pyproject_toml,
        &filenames.uv_lock,
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
        .replace("#%DEFINITIONS%#", &format_definitions(&definitions))
        .replace(
            "#%DEVSHELL_INPUTS%#",
            &format_devshell(&parsed_config.dev_shell),
        );

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

    let flake_nix_changed = write_flake_contents(
        &old_flake_contents,
        &flake_contents,
        use_generated_file_instead,
        &flake_filename,
        flake_dir,
    )?;

    run_git_add(&git_tracked_files, flake_dir)?;
    run_git_commit(flake_dir)?; //after nix 2.23 we will need to commit the flake, possibly. At
                                //least if we wanted to reference it from another flake
    Ok({
        WriteFlakeResult {
            flake_nix_changed,
            python_lock_changed: python_locks_changed,
        }
    })
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

fn format_devshell(dev_shell: &config::TofuDevShell) -> String {
    dev_shell.inputs.join(" ")
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
            .map_or_else(String::new, |x| x.join(" "))),
    )
}

fn add_jupyter_kernels(
    parsed_config: &config::TofuConfigToml,
    definitions: &mut BTreeMap<String, String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
    rust_extensions: &mut Vec<String>,
) {
    let mut jupyter_kernels = String::new();
    let jupyter_included = parsed_config.python.as_ref().is_some_and(|p| {
        p.packages
            .iter()
            .any(|(k, _)| k == "jupyter" || k == "notebook" || k == "jupyterlab")
    });
    if let Some(r) = &parsed_config.r {
        // install R kernel
        if jupyter_included && r.packages.iter().any(|x| x == "IRkernel") {
            jupyter_kernels.push_str(
                "
                mkdir $out/share/jupyter/kernels/R
            cp ${R_tracked}/lib/R/library/IRkernel/kernelspec/* $out/share/jupyter/kernels/R -r
            ",
            );
        }
    }
    if parsed_config
        .nixpkgs
        .packages
        .contains(&"evcxr".to_string())
    {
        jupyter_kernels.push_str(
            "
            JUPYTER_PATH=$out/share/jupyter ${pkgs.evcxr}/bin/evcxr_jupyter --install
        ",
        );
        if !rust_extensions.contains(&"rust-src".to_string()) {
            rust_extensions.push("rust-src".to_string());
        }
    }
    //The python package replaces .../kernel with a symlink.
    //we restore it here.
    if !jupyter_kernels.is_empty() && jupyter_included {
        jupyter_kernels = "
        ln -s ${python_package}/share/jupyter/kernels/python3 $out/share/jupyter/kernels/python3
        "
        .to_string()
            + &jupyter_kernels;
    }
    if jupyter_included && !jupyter_kernels.is_empty() {
        definitions.insert(
            "zzz_jupyter_kernel_drv".to_string(),
            "pkgs.runCommand \"anysnake2-jupyter-kernels\" {} ''
                mkdir -p $out/share/jupyter/kernels
            "
            .to_string()
                + &jupyter_kernels
                + "''",
        );
        //must be in the script so it's done before python.
        //since we go reverse...
        nixpkgs_pkgs.insert("zzz_jupyter_kernel_drv".to_string());
    }
}
struct PrepResult {
    pyproject_fragment: toml::Table,
    writeable_to_nix_store_paths: HashMap<String, String>,
}
/// prepare what we put into pyproject.toml
#[allow(clippy::too_many_lines)]
fn prep_packages_for_pyproject_toml(
    input: &mut HashMap<SafePythonName, config::TofuPythonPackageDefinition>,
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
    pyproject_toml_path: &Path,
) -> Result<PrepResult> {
    let mut result = toml::Table::new();
    let mut writeable_to_nix_store_paths = HashMap::new();
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
                    result.insert(name.to_string(), toml::Value::String(">=0".to_string()));
                } else {
                    result.insert(
                        name.to_string(),
                        toml::Value::String(format!("=={}", version_constraint)),
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
                            &spec.patch_before_lock,
                        )?;
                        writeable_to_nix_store_paths.insert(writeable_path.clone(), path.clone());
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
                        spec.anysnake_override_attrs
                            .get_or_insert_with(|| HashMap::new())
                            .insert("src".to_string(), src.into());
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
                        &spec.patch_before_lock,
                    )?;
                    writeable_to_nix_store_paths.insert(writeable_path.clone(), path.clone());
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
                    spec.anysnake_override_attrs
                        .get_or_insert_with(|| HashMap::new())
                        .insert("src".to_string(), src.into());
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
                    let writeable_path = copy_for_poetry(
                        &path,
                        name,
                        &sha256,
                        pyproject_toml_path,
                        &spec.patch_before_lock,
                    )?;
                    writeable_to_nix_store_paths.insert(writeable_path.clone(), path.clone());
                    out_map.insert("path".to_string(), writeable_path.into());
                    let src = format!(
                        "(
                            pkgs.fetchhg {{
                                    url = \"{url}\";
                                    rev = \"{rev}\";
                                    hash = \"{sha256}\";
                            }})",
                    );
                    spec.anysnake_override_attrs
                        .get_or_insert_with(|| HashMap::new())
                        .insert("src".to_string(), src.into());
                    result.insert(name.to_string(), toml::Value::Table(out_map));
                }
            },
        }
    }
    Ok(PrepResult {
        pyproject_fragment: result,
        writeable_to_nix_store_paths,
    })
}

/// poetry needs *writeable* clones of the repos,
/// because it needs to build egg-infos that write into the checkout
fn copy_for_poetry(
    path: &str,
    name: &SafePythonName,
    sha256: &str,
    pyproject_toml_path: &Path,
    patch_before_lock: &Option<String>,
) -> Result<String> {
    let patch_before_lock_sha = patch_before_lock
        .as_ref()
        .map_or_else(|| "None".to_string(), sha256::digest);

    let target_path = pyproject_toml_path
        .parent()
        .unwrap()
        .join(name.to_string())
        .join(format!("{sha256}-{patch_before_lock_sha}"));
    //copy the full path, using cp...
    if !target_path.exists() {
        ex::fs::create_dir_all(target_path.parent().unwrap())?;
        info!("Copying {} to {}", path, target_path.to_string_lossy());
        let mut cmd = Command::new("cp");
        cmd.args(["-r", path, &target_path.to_string_lossy()]);
        debug!("cmd: {:?}", cmd);
        cmd.status()?;
        // now chmod it to be writeable
        Command::new("chmod")
            .args(["-R", "ug+w", &target_path.to_string_lossy()])
            .status()?;
        info!("Executing prePoetryPatch for {}", name);
        if let Some(patch_before_lock) = patch_before_lock {
            let mut cmd = Command::new("bash")
                .current_dir(&target_path)
                .stdin(Stdio::piped())
                .spawn()?;
            {
                let stdin = cmd.stdin.as_mut().unwrap();
                stdin.write_all(format!("set -xeou pipefail\n{patch_before_lock}").as_bytes())?;
            }
            let output = cmd.wait().context("prePoetryPatch failed")?;
            if output.success() {
                info!("prePoetryPatch succeeded");
            } else {
                bail!("prePoetryPatch failed");
            }
        }
    }
    //I'd love to return these relative, but since we run ancient-poetry in a tmp dir,
    //this will fail.
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

fn nix_format(input: &str, flake_dir: impl AsRef<Path>) -> Result<String> {
    let full_url = format!("{}#nixfmt", anysnake2::get_outside_nixpkgs_url().unwrap());
    // debug!("registering nixfmt with {}", &full_url);
    super::register_nix_gc_root(&full_url, &flake_dir)?;
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
        ex::fs::write(flake_dir.as_ref().join("broken.nix"), input).ok();
        Err(anyhow!(
            "nix fmt error. Broken code in flake_dir/broken. nix return code was {}\n.{}",
            out.status.code().unwrap(),
            input_with_line_nos
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn ancient_poetry(
    ancient_poetry: &vcs::TofuVCS,
    nixpkgs: &config::TofuNixPkgs,
    uv_flake: &vcs::TofuVCS,
    python_packages: &HashMap<SafePythonName, config::TofuPythonPackageDefinition>,
    python_package_definitions: &toml::Table,
    pyproject_toml_path: &Path,
    uv_lock_path: &Path,
    python_version: &str,
    python_major_minor: &str,
    date: jiff::civil::Date,
    uv_env: &Option<HashMap<String, String>>,
) -> Result<()> {
    //let mut pyproject_toml_contents = toml::Table::new();
    //pyproject_toml_contents["tool.poetry"] = toml::Value::Table(toml::Table::new());
    let str_date = date.strftime("%Y-%m-%d").to_string();
    let mut pyproject_toml_contents: toml::Table = format!(
        r#"
[project]
name = "anysnake2-to-ancient-poetry-uv"
version = "0.1.0"
requires-python = "=={python_version}.*"
[tool.ancient-poetry]
ancient-date = "{str_date}"
[tool.uv.sources]
"#
    )
    .parse()
    .unwrap();
    let mut dependencies: Vec<toml::Value> = Vec::new();
    //these we have during locking, but we remove them
    //since they get in via our  venv
    //[tool.poetry.dependencies]
    let uv_sources = pyproject_toml_contents["tool"]["uv"]["sources"]
        .as_table_mut()
        .unwrap();
    for (name, version_constraint) in python_package_definitions {
        match version_constraint {
            toml::Value::String(constraint) => {
                dependencies.push(format!("{name}{}", constraint).into())
            }
            toml::Value::Table(tbl) => {
                dependencies.push(name.to_string().into());
                uv_sources.insert(name.into(), (*tbl).clone().into());
            }
            _ => panic!("unexpected kind of version constraint: {version_constraint:?}"),
        }
    }

    pyproject_toml_contents["project"]
        .as_table_mut()
        .unwrap()
        .insert(
            "description".into(),
            "This file is generated by anysnake2. Do not edit it manually.".into(),
        );
    pyproject_toml_contents["project"]
        .as_table_mut()
        .unwrap()
        .insert("dependencies".into(), toml::Value::Array(dependencies));
    ex::fs::create_dir_all(pyproject_toml_path.parent().unwrap())?;
    let pyproject_contents = pyproject_toml_contents.to_string();
    debug!("Writing {pyproject_toml_path:?}");
    ex::fs::write(pyproject_toml_path, &pyproject_contents)
        .context("Failed to generate pyproject.toml")?;
    let pyproject_toml_hash = sha256::digest(pyproject_contents);

    let last_hash = ex::fs::read_to_string(pyproject_toml_path.with_extension("sha256"))
        .unwrap_or(String::new())
        .trim()
        .to_string();
    if (pyproject_toml_hash != last_hash)
        || !uv_lock_path.exists()
        || uv_lock_path.metadata()?.len() == 0
    {
        //todo make configurable
        let full_url = ancient_poetry.to_nix_string();

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
            format!("{}#uv-bin", uv_flake.to_nix_string()),
            format!("{}#{}", nixpkgs.url.to_nix_string(), python_major_minor),
            full_url,
            "-c".into(),
            "ancient-poetry".into(),
            "--tool".into(),
            "uv".into(),
            "--threshold-date".into(),
            str_date,
            "--pyproject-dot-toml".into(),
            pyproject_toml_path.to_string_lossy().to_string(),
            "--output-filename".into(),
            uv_lock_path.to_string_lossy().to_string(),
        ];
        if !exclusion_list.is_empty() {
            full_args.push("--exclusion-list".into());
            full_args.push(exclusion_list);
        }
        if uv_lock_path.exists() {
            // important, or you'll be dragging in the 'path dependency' of
            // what came before.
            std::fs::remove_file(uv_lock_path)?;
        }
        debug!(
            "running ancient-poetry: nix {}",
            full_args.iter().map(|x| format!("\"{x}\"")).join(" ")
        );
        let out = Command::new("nix")
            .args(full_args)
            .envs(uv_env.as_ref().unwrap_or(&HashMap::new()))
            .current_dir(".")
            //.stdin(Stdio::piped())
            //.stdout(Stdio::piped())
            .status()?;
        if out.success() {
            //write it to poetry.lock
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
            debug!("Flake content change detected (2). writing old flake to flake.nix.old for comparison");
            fs::write(flake_dir.join("flake.nix.old"), old_flake_contents)?;
            fs::write(flake_filename, flake_contents)?;
        }
        Ok(true)
    } else if old_flake_contents != flake_contents {
        debug!("Flake content change detected. writing old flake to flake.nix.old for comparison");
        fs::write(flake_dir.join("flake.nix.old"), old_flake_contents)?;
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
        if !stdout.contains("no changes added") && !stdout.contains("nothing added to commit") {
            let msg = format!("Failed git commit\n Stdout: \n{stdout}\n\nStderr: {stderr}",);
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
        nixpkgs_pkgs.insert("stdenv.cc".to_string()); // needed to actually build something with rust
        let mut out_rust_extensions = vec!["rustfmt".to_string(), "clippy".to_string()];
        out_rust_extensions.extend(rust_extensions);

        inputs.push(InputFlake::new(
            "rust-overlay",
            &rust.url,
            None,
            &["nixpkgs"],
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
            "pkgs.rust-bin.stable.\"{}\".minimal.override {{ extensions = [ {str_rust_extensions}]; }}", rust.version,
        ),
            );
        nixpkgs_pkgs.insert("rust".to_string());
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
            if flake.packages.is_none() {
                nixpkgs_pkgs.insert(format!(
                    "({}.defaultPackage.x86_64-linux or {}.packages.x86_64-linux.defaults)",
                    name, name
                ));
            } else if let Some(pkgs) = &flake.packages {
                for pkg in pkgs {
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

        nixpkgs_pkgs.insert("(builtins.elemAt R.buildInputs 0)".to_string()); // that's the overlayed R.
        nixpkgs_pkgs.insert("R".to_string()); // that's the overlayed R.
    } else {
        definitions.insert("R_tracked".to_string(), "null".to_string());
    }
}

fn format_overrides(
    python_packages: &HashMap<SafePythonName, config::TofuPythonPackageDefinition>,
) -> Result<(Vec<String>, Vec<String>)> {
    fn to_vec(overrides: HashMap<String, HashMap<String, String>>) -> Vec<String> {
        let mut out = Vec::new();
        for (name, key_value) in overrides.into_iter() {
            let mut here = format!("{name} = prev.{name}.overrideAttrs (old: {{");
            for (key, value) in itertools::sorted(key_value) {
                here.push_str(&format!("{key} = {value};"));
            }
            here.push_str("});");
            out.push(here);
        }
        out.sort();
        out
    }

    let mut anysnake_overrides: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut user_overrides: HashMap<String, HashMap<String, String>> = HashMap::new();

    for (name, spec) in python_packages {
        if let Some(build_systems) = &spec.build_systems {
            let target = anysnake_overrides
                .entry(name.to_string())
                .or_insert_with(HashMap::new);
            let str_build_systems = build_systems
                .iter()
                .map(|x| format!("{x} = [];"))
                .collect::<Vec<String>>()
                .join(" ");
            target.insert(
                "nativeBuildInputs".to_string(),
                format!(
                    "old.nativeBuildInputs ++ ( final.resolveBuildSystem {{ {} }} ) ",
                    str_build_systems
                ),
            );
        }
        for (key, value) in spec.override_attrs.iter() {
            let target = user_overrides
                .entry(name.to_string())
                .or_insert_with(HashMap::new);
            target.insert(key.to_string(), value.to_string());
        }
        if let Some(anysnake_override_attrs) = spec.anysnake_override_attrs.as_ref() {
            for (key, value) in anysnake_override_attrs.iter() {
                let target = anysnake_overrides
                    .entry(name.to_string())
                    .or_insert_with(HashMap::new);
                target.insert(key.to_string(), value.to_string());
            }
        }
    }
    Ok((to_vec(anysnake_overrides), to_vec(user_overrides)))
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn add_python(
    parsed_config: &mut config::TofuConfigToml,
    inputs: &mut Vec<InputFlake>,
    definitions: &mut BTreeMap<String, String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
    git_tracked_files: &mut Vec<String>,
    overlays: &mut Vec<String>,
    pyproject_toml_path: &Path,
    uv_lock_path: &Path,
    flake_dir: &Path,
    in_non_spec_but_cached_values: &HashMap<String, String>,
    out_non_spec_but_cached_values: &mut HashMap<String, String>,
) -> Result<bool> {
    //ex::fs::create_dir_all(poetry_lock.parent().unwrap())?;
    let mut changed = false;
    match &mut parsed_config.python {
        Some(python) => {
            let original_pyproject_toml =
                ex::fs::read_to_string(pyproject_toml_path).unwrap_or_else(|_| String::new());
            let original_poetry_lock =
                ex::fs::read_to_string(uv_lock_path).unwrap_or_else(|_| String::new());

            if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                bail!(
                            format!("Python version must be x.y (not x.y.z, z is given by nixpkgs version). Was '{}'", &python.version));
            }
            let python_major_minor = format!("python{}", python.version.replace('.', ""));

            let prep_result = prep_packages_for_pyproject_toml(
                &mut python.packages,
                in_non_spec_but_cached_values,
                out_non_spec_but_cached_values,
                pyproject_toml_path,
            )?;
            let mut out_python_packages = prep_result.pyproject_fragment;
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
                &parsed_config.uv2nix.source,
                &python.packages,
                &out_python_packages,
                pyproject_toml_path,
                uv_lock_path,
                &python_version,
                &python_major_minor,
                ecosystem_date,
                &python.uv_lock_env,
            )?;

            rewrite_poetry(flake_dir, &prep_result.writeable_to_nix_store_paths)?;
            let extend_build_systems = check_if_setuptools_needs_expansion(uv_lock_path)?;
            write_setup_cfg(flake_dir, ecosystem_date, git_tracked_files)?;

            let (local_anysnake_overrides, local_user_overrides) = //todo: override_attrs...
                format_overrides(&python.packages)?;

            inputs.push(InputFlake::new(
                "uv2nix",
                &parsed_config.uv2nix.source,
                None,
                &[],
            ));
            inputs.push(InputFlake::new(
                "
            pyproject-build-systems",
                &parsed_config.pyproject_build_systems,
                None,
                &["uv2nix", "nixpkgs"],
            ));
            inputs.push(InputFlake::new(
                "uv2nix_override_collection",
                &parsed_config.uv2nix_override_collection,
                None,
                &[],
            ));

            let prefer_wheels = parsed_config.uv2nix.prefer_wheels;
            let source_preference = if prefer_wheels { "wheel" } else { "sdist" };
            definitions.insert(
                "pyproject-nix".to_string(),
                "uv2nix.inputs.pyproject-nix".to_string(),
            );
            let workspace = "uv_rewritten";
            definitions.insert(
                "workspace".to_string(),
                format!(
                    "uv2nix.lib.workspace.loadWorkspace {{workspaceRoot = ./{};}}",
                    workspace
                ),
            );

            definitions.insert(
                "overlay".to_string(),
                format!("workspace.mkPyprojectOverlay {{ sourcePreference = \"{source_preference}\"; }}")
            );

            definitions.insert(
                "fix_resolve_build_systems".to_string(),
                (if extend_build_systems {
                    "
                    (final: prev: {
                        resolveBuildSystem =
                        arg: prev.resolveBuildSystem (arg
                        //
                        # only if setuptools is in arg
                        (
                          if arg.setuptools or null != null
                          then {wheel = [];}
                          else {}
                        ));
                    })"
                } else {
                    "(final: prev: {} )"
                })
                .to_string(),
            );

            definitions.insert(
                "local_anysnake_overrides".to_string(),
                format!(
                    "(final: prev: {{ {} }})",
                    local_anysnake_overrides.join("\n")
                ),
            );
            definitions.insert(
                "local_user_overrides".to_string(),
                format!("(final: prev: {{ {} }})", local_user_overrides.join("\n")),
            );

            definitions.insert(
                "pyprojectOverrides ".to_string(),
                "[ fix_resolve_build_systems
                    (uv2nix_override_collection.overrides pkgs)
                    local_anysnake_overrides
                    local_user_overrides
                ]"
                .to_string(),
            ); //todo: insert override_attrs here.
            definitions.insert(
                "interpreter".to_string(),
                format!("pkgs.{}", python_major_minor),
            );
            definitions.insert(
                "spec".to_string(),
                "{anysnake2-to-ancient-poetry-uv = []; }".to_string(),
            );
            // Use base package set from pyproject.nix builders
            definitions.insert(
                "pythonSet".to_string(),
                "(pkgs.callPackage pyproject-nix.build.packages {
                    python = interpreter;
        }).overrideScope
          (
            pkgs.lib.composeManyExtensions ([
              pyproject-build-systems.overlays.default
              overlay ] ++ 
              pyprojectOverrides)
          )"
                .to_string(),
            );
            //Override host packages with build fixups
            /* definitions.insert(
                "pythonSet".to_string(),
                "pythonSet'.pythonPkgsHostHost.overrideScope pyprojectOverrides".to_string(),
            ); */

            definitions.insert(
                "python_package".to_string(),
                format!("pythonSet.mkVirtualEnv \"anysnake2-venv\" spec"),
            );
            nixpkgs_pkgs.insert("python_package".to_string());

            git_tracked_files.push("uv_rewritten/uv.lock".to_string());
            git_tracked_files.push("uv_rewritten/pyproject.toml".to_string());
            let new_pyproject_toml =
                ex::fs::read_to_string(pyproject_toml_path).unwrap_or_else(|_| String::new());
            let new_poetry_lock =
                ex::fs::read_to_string(uv_lock_path).unwrap_or_else(|_| String::new());
            if new_pyproject_toml != original_pyproject_toml
                || new_poetry_lock != original_poetry_lock
            {
                changed = true;
            }

            overlays.push(
                "(final: prev: {
                   uv = uv2nix.packages.\"${system}\".uv-bin;
            })"
                .to_string(),
            );
        }
        None => {
            if uv_lock_path.exists() {
                ex::fs::remove_file(uv_lock_path)?;
                changed = true;
            }
        }
    };

    Ok(changed)
}

pub struct PrefetchResult {
    pub path: String,
    pub sha256: String,
}

pub fn prefetch_hg_store_path(url: &str, rev: &str) -> Result<PrefetchResult> {
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

pub fn prefetch_git_store_path(url: &str, rev: &str) -> Result<PrefetchResult> {
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

pub fn prefetch_github_store_path(url: &str, rev: &str) -> Result<PrefetchResult> {
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
        let nix_code = format!(
            "pkgs.fetchFromGitHub {{
                    owner = \"{owner}\";
                    repo = \"{repo}\";
                    rev = \"{rev}\";
                    sha256 = \"{new_sha}\";
                  }}
            "
        );

        std::fs::write(
            &default_nix,
            format!(
                "
                {{ pkgs ? import <nixpkgs> {{}} }}:

                {nix_code}
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
/// we need it to find them in the nix store, but for the locking we needed them outside.
fn rewrite_poetry(
    flake_dir: &Path,
    writeable_to_nix_store_paths: &HashMap<String, String>,
) -> Result<()> {
    ex::fs::create_dir_all(flake_dir.join("uv_rewritten"))?;

    let filename = "pyproject.toml";
    let input_filename = flake_dir.join("uv").join(filename);
    let output_filename = flake_dir.join("uv_rewritten").join(filename);
    let raw = ex::fs::read_to_string(input_filename).context("rewrite_poetry")?;
    let mut out = raw;
    for (search, replace) in writeable_to_nix_store_paths {
        out = out.replace(search, replace);
    }
    ex::fs::write(output_filename, out)?;

    let str_flake_dir = flake_dir.canonicalize()?.to_string_lossy().to_string();
    dbg!(&str_flake_dir);

    let filename = "uv.lock";
    let input_filename = flake_dir.join("uv").join(filename);
    let output_filename = flake_dir.join("uv_rewritten").join(filename);
    let raw = ex::fs::read_to_string(input_filename).context("Rewrite_poetry")?;
    let mut out = raw;
    for (search, replace) in writeable_to_nix_store_paths {
        let search_minus_slash = search
            .strip_prefix("/")
            .unwrap_or(search)
            .replace('+', "[+]");
        let re = regex::Regex::new(&format!(r"([.][.]/)+{}", search_minus_slash)).unwrap();
        let replace_with = format!("../../../..{replace}"); //must be relative to the nix store.
        out = re.replace_all(&out, replace_with).to_string();
    }
    ex::fs::write(output_filename, out)?;

    Ok(())
}

/// old setuptools can't read pyprojec.toml, leading to build errors
/// for 'unknown metadata Name.
/// this places a dummy setup.cfg in our generated top level project
fn write_setup_cfg(
    flake_dir: &Path,
    ecosystem_date: jiff::civil::Date,
    git_tracked_files: &mut Vec<String>,
) -> Result<()> {
    if ecosystem_date <= jiff::civil::Date::constant(2022, 3, 24) {
        let setup_cfg = flake_dir.join("uv_rewritten/setup.cfg");
        ex::fs::write(
            setup_cfg,
            "[metadata]\nname = anysnake2-to-ancient-poetry-uv\nversion = 0.1.0\n",
        )?;
        git_tracked_files.push("uv_rewritten/setup.cfg".to_string());
    }
    Ok(())
}

/// setuptools before 70.1.1 needs us to add 'wheel' every time we see setuptools in a buildSystems
fn check_if_setuptools_needs_expansion(uv_lock_path: &Path) -> Result<bool> {
    let parsed_uv_lock: toml::Table =
        toml::from_str(&ex::fs::read_to_string(uv_lock_path).context("Failed to read uv.lock")?)
            .context("Failed to parse uv.lock")?;
    for pkg_info in parsed_uv_lock["package"]
        .as_array()
        .context("No package sections in uv.lock?")?
    {
        let name = pkg_info["name"]
            .as_str()
            .context("No name in package section")?;
        if name == "setuptools" {
            let version = pkg_info["version"]
                .as_str()
                .context("No version in package section for setuptools")?;
            let mut ver = version.split(".");
            let start: u32 = ver
                .next()
                .context("Failed to parse setuptools version")?
                .parse()
                .context("Failed to parse setuptools version")?;
            if start < 70 {
                return Ok(true);
            } else if start == 70 {
                let second: u32 = ver
                    .next()
                    .context("Failed to parse setuptools version")?
                    .parse()
                    .context("Failed to parse setuptools version")?;
                if second < 1 {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
    }

    Ok(false)
}
