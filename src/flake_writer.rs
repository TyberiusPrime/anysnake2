use crate::config::{self, BuildPythonPackageInfo};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use ex::fs;
use itertools::Itertools;
use log::{debug, trace};
use regex::Regex;
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{python_parsing, run_without_ctrl_c};

/// captures everything we need to know about an 'input' to our flake.
struct InputFlake {
    name: String,
    url: String,
    rev: String,
    follows: Vec<String>,
    is_flake: bool,
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
            is_flake: true,
        })
    }
    fn new_with_flake_option(
        name: &str,
        url: &str,
        rev: &str,
        follows: &[&str],
        flake_dir: impl AsRef<Path>,
        is_flake: bool,
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
            is_flake,
        })
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

#[allow(clippy::vec_init_then_push)]
pub fn write_flake(
    flake_dir: impl AsRef<Path>,
    parsed_config: &mut config::ConfigToml,
    python_packages: &[(String, String)],
    python_build_packages: &HashMap<String, BuildPythonPackageInfo>, // those end up as buildPythonPackages
    use_generated_file_instead: bool, // which is set if do_not_modify_flake is in effect.
) -> Result<bool> {
    let template = std::include_str!("nix/flake_template.nix");
    let flake_dir: &Path = flake_dir.as_ref();

    let filenames = get_filenames(&flake_dir, use_generated_file_instead);
    let flake_filename = filenames.flake_filename;
    //let poetry_lock = filenames.poetry_lock;
    //let pyproject_toml = filenames.pyproject_toml;
    let old_flake_contents = fs::read_to_string(&flake_filename).unwrap_or_else(|_| "".to_string());
    //let old_poetry_lock = fs::read_to_string(&poetry_lock).unwrap_or_else(|_| "".to_string());

    let mut flake_contents: String = template.to_string();

    //various 'collectors'
    let mut inputs: Vec<InputFlake> = Vec::new();
    let mut definitions: HashMap<String, String> = HashMap::new();
    let mut overlays: Vec<String> = Vec::new();
    let mut rust_extensions: Vec<String> = Vec::new();
    let mut flakes_used_for_python_packages: HashSet<String> = HashSet::new();
    let mut nixpkgs_pkgs = BTreeSet::new();
    //let mut nix_pkg_overlays = Vec::new();

    // we always need the flake utils.
    inputs.push(InputFlake::new(
        "flake-utils",
        &parsed_config.flake_util.url,
        &parsed_config.flake_util.rev,
        &[],
        flake_dir,
    )?);

    // and nixpkgs is non optional as well.

    inputs.push(InputFlake::new(
        "nixpkgs",
        &parsed_config.nixpkgs.url,
        &parsed_config.nixpkgs.rev,
        &[],
        flake_dir,
    )?);
    nixpkgs_pkgs.extend(match &parsed_config.nixpkgs.packages {
        Some(pkgs) => pkgs.clone(),
        None => Vec::new(),
    });
    nixpkgs_pkgs.insert("cacert".to_string()); //so we have SSL certs inside
                                               //
    if let Some(rust_ver) = &parsed_config.rust.version {
        add_rust(
            &rust_ver.clone(),
            rust_extensions,
            &mut inputs,
            &mut definitions,
            &mut nixpkgs_pkgs,
            &mut overlays,
            &flake_dir,
            parsed_config,
        )?;
    }

    add_flakes(
        parsed_config,
        &mut inputs,
        &flake_dir,
        &flakes_used_for_python_packages,
        &mut nixpkgs_pkgs
        )?;

    /*     let mut flakes_used_for_python_packages = HashSet::new(); */

    //ex::fs::create_dir_all(poetry_lock.parent().unwrap())?;
    /* flake_contents = match &parsed_config.python {
            Some(python) => {
                if !Regex::new(r"^\d+\.\d+$").unwrap().is_match(&python.version) {
                    bail!(
                            format!("Python version must be x.y (not x.y.z ,z is given by nixpkgs version). Was '{}'", &python.version));
                }
                let python_major_dot_minor = &python.version;
                let python_major_minor = format!("python{}", python.version.replace(".", ""));

                let mut out_python_packages = extract_non_editable_python_packages(
                    python_packages,
                    python_build_packages,
                    &parsed_config.flakes,
                )?;
                if parsed_config.r.is_some() {
                    out_python_packages.push(("rpy2".to_string(), "".to_string()));
                }
                out_python_packages.sort();

                ancient_poetry(
                    &out_python_packages,
                    &pyproject_toml,
                    &poetry_lock,
                    &python.version,
                    python.parsed_ecosystem_date()?,
                )?;

                let out_python_packages = "remove_me".to_string();

                let out_python_build_packages = format_python_build_packages(
                    &python_build_packages,
                    &parsed_config.flakes,
                    &mut flakes_used_for_python_packages,
                )?;

                if let Some(_org) = &python.additional_mkpython_arguments {
                    bail!("python.additional_mkpython_arguments requirements is not supported anymore (merge issues with '_'). \nUse addition_mk_python_arguments_func like this '''
    old: old // {{\"_\"  = old.\"_\" // {{
        pandas.postInstall = ''
            touch $out/lib/python* /site-packages/pandas_mkpython_worked

        '';
        }};
        }}
    '''");
                }

                let out_additional_mkpython_arguments_func = &python
                    .additional_mkpython_arguments_func
                    .as_deref()
                    .unwrap_or("old: old");

                let ecosystem_date = python
                    .parsed_ecosystem_date()
                    .context("Failed to parse python.ecosystem-date")?;
                let pypi_debs_db_rev = pypi_deps_date_to_rev(ecosystem_date, &flake_dir)?;

                inputs.push(InputFlake::new(
                    "poetry2nix",
                    &parsed_config.poetry2nix.url,
                    &parsed_config.poetry2nix.rev,
                    &[],
                    &flake_dir,
                )?);

                flake_contents
                    //.replace("%PYTHON_MAJOR_MINOR%", &python_major_minor)
                    .replace("%PYTHON_PACKAGES%", &out_python_packages)
                    .replace("PYTHON_BUILD_PACKAGES", &out_python_build_packages)
                    .replace(
                        "PYTHON_ADDITIONAL_MKPYTHON_ARGUMENTS_FUNC",
                        out_additional_mkpython_arguments_func,
                    )
                    .replace("%PYTHON_MAJOR_DOT_MINOR%", &python_major_dot_minor)
                    .replace("%PYPI_DEPS_DB_REV%", &pypi_debs_db_rev)
                    .replace(
                        "\"%POETRY2NIX%\";",
                        "inherit (poetry2nix.lib.mkPoetry2Nix {inherit pkgs;}) mkPoetryEnv defaultPoetryOverrides;"
                    )
            }
            None => {
                if poetry_lock.exists() {
                    fs::remove_file(poetry_lock)?;
                }
                flake_contents
                    .replace("\"%POETRY2NIX%\";", "mkPoetryEnv = null;")
                    .replace("%DEVELOP_PYTHON_PATH%", "")
                    .replace("#%PYTHON_BUILD_PACKAGES%", "")
                    .replace("#%PYTHON_ADDITIONAL_MKPYTHON_ARGUMENTS%", "")
            }
        }; */

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

    /* flake_contents = match &parsed_config.r {
        Some(r_config) => {
            inputs.push(InputFlake::new(
                "nixR",
                &r_config.nixr_url,
                &r_config
                    .nixr_tag
                    .as_ref()
                    .expect("No nixR tag? Should have been lookup up automatically. Anysnake2 bug"),
                &[],
                &flake_dir,
            )?);

            fn attrset_from_hashmap(attrset: &HashMap<String, String>) -> String {
                let mut out = "".to_string();
                for (pkg_name, override_nix_func) in attrset.iter() {
                    out.push_str(&format!("\"{}\" = ({});", pkg_name, override_nix_func));
                }
                out
            }

            let r_override_args = r_config
                .override_attrs
                .as_ref()
                .map_or("".to_string(), attrset_from_hashmap);
            let r_dependency_overrides = r_config
                .dependency_overrides
                .as_ref()
                .map_or("".to_string(), attrset_from_hashmap);
            let r_additional_packages = r_config
                .additional_packages
                .as_ref()
                .map_or("".to_string(), attrset_from_hashmap);

            let mut r_pkg_list: Vec<String> =
                r_config.packages.iter().map(|x| x.to_string()).collect();
            if let Some(additional_packages) = &r_config.additional_packages {
                for pkg_ver in additional_packages.keys() {
                    let (pkg, _ver) = pkg_ver.split_once("_").expect(
                        "R.additional_packages key did not conform to 'name_version' schema",
                    );
                    r_pkg_list.push(pkg.to_string());
                }
            }
            //remove duplicates
            r_pkg_list.sort();
            r_pkg_list.dedup();

            let r_packages = format!(
                "
                R_tracked = nixR.R_by_date {{
                    date = \"{}\" ;
                    r_pkg_names = [{}];
                    packageOverrideAttrs = {{ {} }};
                    r_dependency_overrides = {{ {} }};
                    additional_packages = {{ {} }};
                }};
                ",
                &r_config.date,
                r_pkg_list.iter().map(|x| format!("\"{}\"", x)).join(" "),
                r_override_args,
                r_dependency_overrides,
                r_additional_packages
            );
            overlays.push(
                "(final: prev: {
                R = R_tracked // {meta = { platforms=prev.R.meta.platforms;};};
                rPackages = R_tracked.rPackages;
                }) "
                .to_string(),
            );

            nixpkgs_pkgs.push("R".to_string()); // that's the overlayed R.
            flake_contents.replace("#%RPACKAGES%", &r_packages)
        }
        None => flake_contents.replace("#%RPACKAGES%", "R_tracked = null;"),
    }; */
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
    flake_contents = insert_allow_unfree(&flake_contents, parsed_config.nixpkgs.allow_unfree);

    flake_contents = flake_contents
        .replace("#%INPUT_DEFS%", &format_input_defs(&inputs))
        .replace("#%INPUTS%", &format_inputs_for_output_arguments(&inputs))
        .replace("#%DEFINITIONS%#", &format_definitions(&definitions));

    // pretty print the generated flake
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

    /* if !overlays.is_empty() {
        flake_contents = flake_contents.replace(
            "\"%OVERLAY_AND_PACKAGES%\"",
            &("[".to_string() + &overlays.join(" ") + "]"),
        );
    } else {
        flake_contents = flake_contents.replace("\"%OVERLAY_AND_PACKAGES%\"", "[]");
    } */

    //print!("{}", flake_contents);
    let git_tracked_files = create_flake_git(flake_dir.as_ref())?;
    /*
    if parsed_config.python.is_some() {
        gitargs.push("poetry/pyproject.toml");
        gitargs.push("poetry/poetry.lock");
    } */

    let res = write_flake_contents(
        &old_flake_contents,
        &flake_contents,
        use_generated_file_instead,
        &flake_filename,
        flake_dir.as_ref(),
    )?;

    run_git_add(git_tracked_files, flake_dir)?;

    Ok(res)
}

/// format the list of input flakes for the inputs = {} section of a flake.nix
fn format_input_defs(inputs: &[InputFlake]) -> String {
    let mut out = "".to_string();
    for fl in inputs {
        let v_follows: Vec<String> = fl
            .follows
            .iter()
            .map(|x| format!("        inputs.{}.follows = \"{}\";", &x, &x))
            .collect();
        let str_follows = v_follows.join("\n");
        let url = if fl.url.starts_with("github") && fl.url.matches("/").count() == 3 {
            //has a branch - but we define a revision, and you can't have both for some reason
            let mut iter = fl.url.rsplitn(2, "/");
            iter.next(); // eat the branch
            iter.collect()
        } else {
            fl.url.to_string()
        };
        out.push_str(&format!(
            "
    {} = {{
        url = \"{}{}rev={}\";
{}
{}
    }};",
            fl.name,
            url,
            if !fl.url.contains("?") { "?" } else { "&" },
            fl.rev,
            &str_follows,
            if fl.is_flake { "" } else { "flake = false;" }
        ))
    }
    out
}

/// format the list of input flakes for the outputs {self, <arguments>}
fn format_inputs_for_output_arguments(inputs: &[InputFlake]) -> String {
    inputs.iter().map(|i| &i.name[..]).join(",\n    ")
}

fn format_definitions(definitions: &HashMap<String, String>) -> String {
    let mut res = "".to_string();
    for (k, v) in definitions {
        res.push_str(&format!("     {} = {};\n", k, v));
    }
    res.trim().to_string()
}

fn insert_nixpkgs_pkgs(flake_contents: &str, nixpkgs_pkgs: &BTreeSet<String>) -> String {
    let str_nixpkgs_pkgs: String = nixpkgs_pkgs
        .iter()
        .map(|x| format!("${{{}}}", x))
        .collect::<Vec<String>>()
        .join("\n");
    flake_contents.replace("#%NIXPKGS_PACKAGES%#", &str_nixpkgs_pkgs)
}

fn insert_allow_unfree(flake_contents: &str, allow_unfree: bool) -> String {
    flake_contents.replace(
        "\"%ALLOW_UNFREE%\"",
        if allow_unfree { "true" } else { "false" },
    )
}

fn extract_non_editable_python_packages(
    input: &[(String, String)],
    build_packages: &HashMap<String, BuildPythonPackageInfo>,
    flakes_config: &Option<HashMap<String, config::Flake>>,
) -> Result<Vec<(String, String)>> {
    let mut res: Vec<(String, String)> = Vec::new();
    for (name, version_constraint) in input.iter() {
        if version_constraint.starts_with("editable") {
            continue;
        }
        if build_packages.contains_key(name) {
            continue; // added below
        } else if version_constraint.contains("==")
            || version_constraint.contains('>')
            || version_constraint.contains('<')
            || version_constraint.contains('!')
        {
            res.push((name.to_string(), version_constraint.to_string()));
        } else if version_constraint.contains('=') {
            res.push((name.to_string(), version_constraint.to_string()));
        } else if version_constraint.is_empty() {
            res.push((name.to_string(), ">0".to_string()))
        } else {
            res.push((name.to_string(), version_constraint.to_string()));
            //bail!("invalid python version spec {}{}", name, version_constraint);
        }
    }
    for (name, spec) in build_packages.iter() {
        let rev_override = match spec.get("method") {
            Some(method) => {
                if method == "useFlake" {
                    let flake_name = spec.get("flake_name").unwrap_or(name);
                    Some(get_flake_rev(flake_name, flakes_config).with_context(|| {
                        format!("no flake revision in flake {} used by {}", flake_name, name)
                    })?)
                } else {
                    None
                }
            }
            _ => None,
        };
        /* res.push((
            name.to_string(),
            python_version_from_spec(spec, rev_override.as_deref()),
        )); */
    }
    Ok(res)
}

fn python_version_from_spec(
    spec: &BuildPythonPackageInfo,
    override_version: Option<&str>,
) -> String {
    format!(
        "999+{}",
        override_version.unwrap_or(
            spec.get("version")
                .unwrap_or(spec.get("rev").unwrap_or(&"0+unknown_version".to_string()))
        )
    )
}

fn get_flake_rev(
    flake_name: &str,
    flakes_config: &Option<HashMap<String, config::Flake>>,
) -> Result<String> {
    Ok(flakes_config
        .as_ref()
        .context("no flakes defined")?
        .get(flake_name)
        .with_context(|| format!("No flake {} in flake definitions", flake_name))?
        .rev
        .as_ref()
        .with_context(|| format!("No rev for flake {} in flake definitions", flake_name))?
        .to_string())
}

fn format_python_build_packages(
    input: &HashMap<String, BuildPythonPackageInfo>,
    flakes_config: &Option<HashMap<String, config::Flake>>,
    flakes_used_for_python_packages: &mut HashSet<String>,
) -> Result<String> {
    let mut res: String = "".into();
    let mut providers: String = "".into();
    let mut packages_extra: Vec<String> = Vec::new();
    for (key, spec) in input.iter().sorted_by_key(|x| x.0) {
        let overrides = match &spec.overrides {
            Some(ov_packages) => {
                let mut ov = "overridesPre = [ (self: super: { ".to_string();
                for p in ov_packages {
                    ov.push_str(&format!("{} = {}_pkg;\n", p, p));
                }

                ov.push_str(" } ) ];");
                ov
            }
            None => "".to_string(),
        };
        match spec
            .get("method")
            .expect("no method in package definition")
            .as_str()
        {
            "useFlake" => {
                let flake_name = spec.get("flake_name").unwrap_or(key);
                let flake_rev = get_flake_rev(flake_name, flakes_config)
                    .with_context(|| format!("python.packages.{}", key))?;
                res.push_str(&format!(
                    "{}_pkg = ({}.mach-nix-build-python-package pkgs mach-nix_ \"{}\");\n",
                    //todo: shohorn the overrides into this?!
                    flake_name,
                    flake_name,
                    python_version_from_spec(spec, Some(&flake_rev))
                ));
                flakes_used_for_python_packages.insert(flake_name.to_string());
                packages_extra.push(flake_name.to_string());
            }
            _ => {
                res.push_str(&format!(
                    "{key}_pkg = prev.{key}.override rec {{
                pname = \"{key}\";
                version=\"{version}\";
                src = {src_method} {{ # {src_comment}
                    {src_spec}
                }};
                {arguments}
              {overrides}
              }});\n",
                    key = key,
                    version = python_version_from_spec(&spec, None),
                    src_method = match spec
                        .get("method")
                        .expect("Missing 'method' on python build package definition")
                        .as_ref()
                    {
                        "fetchPypi" => "pkgs.python3Packages.fetchPypi".to_string(),
                        other => format!("pkgs.{other}"),
                    },
                    src_comment = key,
                    src_spec = spec.src_to_nix(),
                    arguments = spec //todo: handle this...
                        .get("buildPythonPackage_arguments")
                        .map(|str_including_curly_braces| str_including_curly_braces
                            .trim()
                            .trim_matches('{')
                            .trim_matches('}')
                            .trim())
                        .unwrap_or(""),
                    overrides = overrides
                ));
                packages_extra.push(key.to_string());
            }
        }
        providers.push_str(&format!("providers.{} = \"nixpkgs\";\n", key));
    }

    let mut out: String = "// (let ".into();
    out.push_str(&res);
    out.push_str("machnix_overrides = (self: super: {");
    for pkg in packages_extra.iter() {
        out.push_str(&format!("{} = {}_pkg;\n", pkg, pkg));
    }
    out.push_str("} );\n");
    out.push_str("in { packagesExtra = [");
    for pkg in packages_extra.iter() {
        out.push_str(pkg);
        out.push_str("_pkg ");
    }
    out.push_str("]");
    out.push_str("\n; overridesPre = [ machnix_overrides ];\n");
    out.push_str("\n");
    out.push_str(&providers);
    out.push_str("})\n");
    Ok(out)
}

fn pypi_deps_date_to_rev(date: NaiveDate, flake_dir: impl AsRef<Path>) -> Result<String> {
    let query_date = date.and_hms_opt(0, 0, 0).unwrap();
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
    use base64::Engine;
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
                    if page < 0 {
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

pub fn get_proxy_req() -> Result<ureq::Agent> {
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

pub(crate) fn get_github_tags(repo: &str, page: i32) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://api.github.com/repos/{}/tags?per_page=100&page={}",
        repo, page
    );
    debug!("Retrieving {}", &url);
    let body: String = add_auth(get_proxy_req()?.get(&url)).call()?.into_string()?;
    let json: serde_json::Value =
        serde_json::from_str(&body).context("Failed to parse github tags api")?;
    Ok(json
        .as_array()
        .context("No entries in github tags api?")?
        .to_owned())
}

impl Retriever for GitHubTagRetriever {
    fn retrieve(&self) -> Result<HashMap<String, String>> {
        let mut res = HashMap::new();
        for page in 0..30 {
            let json = get_github_tags(&self.repo, page)?;
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
    dbg!(&full_args);
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

fn ancient_poetry(
    python_package_definitions: &Vec<(String, String)>,
    pyproject_toml_path: &Path,
    poetry_lock_path: &Path,
    python_version: &str,
    date: chrono::NaiveDate,
) -> Result<()> {
    let mut pyproject_contents = r#"
[tool.poetry]
name = "anysnake2_to_ancient_poetry"
version = "0.1.0"
description = ""
authors = ["Nemo"]

[build-system]
requires = ["poetry-core"]
build-backend = "poetry.core.masonry.api"

[tool.poetry.dependencies]

"#
    .to_string();
    pyproject_contents.push_str(&format!("python= \"^{}\"\n", python_version));
    for (name, version_constraint) in python_package_definitions.iter() {
        pyproject_contents.push_str(&format!("{} = \"{}\"\n", name, version_constraint));
    }
    ex::fs::write(pyproject_toml_path, &pyproject_contents)?;
    let pyproject_toml_hash = sha256::digest(pyproject_contents);

    let last_hash = ex::fs::read_to_string(poetry_lock_path.with_extension("sha256"))
        .unwrap_or("".to_string())
        .trim()
        .to_string();
    if (pyproject_toml_hash != last_hash) || !poetry_lock_path.exists() {
        //todo make configurable
        let full_url = format!("git+https://codeberg.org/TyberiusPrime/ancient-poetry.git");
        let str_date = date.format("%Y-%m-%d").to_string();

        let full_args = vec![
            "shell".to_string(),
            full_url,
            "-c".into(),
            "ancient-poetry".into(),
            "-t".into(),
            str_date,
            "-p".into(),
            pyproject_toml_path.to_string_lossy().to_string(),
        ];
        debug!("running ancient-poetry");
        let child = Command::new("nix")
            .args(full_args)
            //.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let out = child
            .wait_with_output()
            .context("Failed to wait on nixfmt")?; // closes stdin
        if out.status.success() {
            let stdout = std::str::from_utf8(&out.stdout)
                .context("ancient-poetry lock output wan't utf8")?;
            //write it to poetry.lock
            ex::fs::write(poetry_lock_path, stdout)?;
            ex::fs::write(
                poetry_lock_path.with_extension("sha256"),
                pyproject_toml_hash,
            )?;
            Ok(())
        } else {
            Err(anyhow!(
                "ancient-poetry error return{}\n",
                out.status.code().unwrap(),
            ))
        }
    } else {
        Ok(())
    }
}

fn create_flake_git(flake_dir: &Path) -> Result<Vec<&str>> {
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

    let mut gitargs = vec!["flake.nix", "functions.nix", ".gitignore"];
    if flake_dir.join("flake.lock").exists() {
        gitargs.push("flake.lock");
    }
    fs::write(
        flake_dir.join(".gitignore"),
        "result
    run_scripts/
    .*.json
    .gc_roots
    ",
    )?;

    Ok(gitargs)
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
        fs::write(&flake_filename, flake_contents)
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

fn run_git_add(tracked_files: Vec<&str>, flake_dir: &Path) -> Result<()> {
    let output = run_without_ctrl_c(|| {
        Command::new("git")
            .arg("add")
            .args(&tracked_files)
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
    Ok(())
}
fn add_rust(
    rust_ver: &str,
    rust_extensions: Vec<String>,
    inputs: &mut Vec<InputFlake>,
    definitions: &mut HashMap<String, String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
    overlays: &mut Vec<String>,
    flake_dir: &Path,
    parsed_config: &mut config::ConfigToml,
) -> Result<()> {
    nixpkgs_pkgs.insert("stdenv.cc".to_string()); // needed to actually build something with rust
    let mut out_rust_extensions = vec!["rustfmt".to_string(), "clippy".to_string()];
    out_rust_extensions.extend(rust_extensions);

    inputs.push(InputFlake::new(
        "rust-overlay",
        &parsed_config.rust.rust_overlay_url,
        &parsed_config.rust.rust_overlay_rev,
        &["nixpkgs", "flake-utils"],
        &flake_dir,
    )?);
    overlays.push("import rust-overlay".to_string());
    let str_rust_extensions: Vec<String> = out_rust_extensions
        .into_iter()
        .map(|x| format!("\"{}\"", x))
        .collect();
    let str_rust_extensions: String = str_rust_extensions.join(" ");

    definitions.insert(
        "rust".to_string(),
        format!(
            "pkgs.rust-bin.stable.\"{}\".minimal.override {{ extensions = [ {rust_extensions}]; }}",
            rust_ver,
            rust_extensions = str_rust_extensions
        ),
    );
    nixpkgs_pkgs.insert("rust".to_string());
    Ok(())
}

fn add_flakes(
    parsed_config: &mut config::ConfigToml,
    inputs: &mut Vec<InputFlake>,
    flake_dir: &Path,
    flakes_used_for_python_packages: &HashSet<String>,
    nixpkgs_pkgs: &mut BTreeSet<String>,
) -> Result<()> {
    {
        if let Some(flakes) = &parsed_config.flakes {
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
                    flake.rev.as_ref().unwrap(), // at this point we must have a rev,
                    &rev_follows[..],
                    &flake_dir,
                )?);
                match &flake.packages {
                    Some(pkgs) => {
                        for pkg in pkgs {
                            nixpkgs_pkgs.insert(format!("{}.{}", name, pkg));
                        }
                    }
                    None => {
                        if !flakes_used_for_python_packages.contains(name) {
                            nixpkgs_pkgs.insert(format!(
                                "{}.{}",
                                name, "defaultPackage.x86_64-linux"
                            ));
                        } //else $default to no packages for a python package flake
                    }
                }
            }
        }
    }
    Ok(())
}
