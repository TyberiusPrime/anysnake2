#![allow(unused_imports, unused_variables, unused_mut, dead_code)] // todo: remove
use crate::vcs::{ParsedVCS, TofuVCS};
use anyhow::{bail, Context, Result};
use itertools::{all, Itertools};
use log::{debug, info};
use serde::de::Deserializer;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub trait GetRecursive {
    fn get_recursive(&self, key: &[&str]) -> Option<&toml::Value>;
}

impl GetRecursive for toml::Value {
    fn get_recursive(&self, key: &[&str]) -> Option<&toml::Value> {
        let mut current = self;
        for k in key {
            current = current.get(k)?;
        }
        Some(current)
    }
}
impl GetRecursive for toml::Table {
    fn get_recursive(&self, key: &[&str]) -> Option<&toml::Value> {
        if let Some(hit) = self.get(key[0]) {
            if key.len() > 1 {
                match hit {
                    toml::Value::Table(t) => t.get_recursive(&key[1..]),
                    _ => None,
                }
            } else {
                Some(hit)
            }
        } else {
            None
        }
    }
}

impl GetRecursive for Option<&toml::Value> {
    fn get_recursive(&self, key: &[&str]) -> Option<&toml::Value> {
        self.and_then(|start| {
            let mut current = start;
            for k in key {
                current = current.get(k)?;
            }
            Some(current)
        })
    }
}

//just enough to read the requested version
#[derive(Deserialize, Debug)]
pub struct MinimalConfigToml {
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: Option<Anysnake2>,
}

#[derive(Debug)]
pub struct TofuMinimalConfigToml {
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: TofuAnysnake2,
}

#[derive(Deserialize, Debug)]
pub struct ConfigToml {
    #[serde(skip)]
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: Anysnake2,
    pub nixpkgs: Option<NixPkgs>,
    pub outside_nixpkgs: Option<ParsedVCSInsideURLTag>,
    pub ancient_poetry: Option<ParsedVCSInsideURLTag>,
    pub poetry2nix: Option<ParsedVCSInsideURLTag>,
    #[serde(default, rename = "flake-util")]
    pub flake_util: Option<ParsedVCSInsideURLTag>,
    pub clone_regexps: Option<HashMap<String, String>>,
    pub clones: Option<HashMap<String, HashMap<String, ParsedVCS>>>,
    #[serde(default)]
    pub cmd: HashMap<String, Cmd>,
    pub rust: Option<Rust>,
    pub python: Option<Python>,
    #[serde(default)]
    pub container: Container,
    pub flakes: Option<HashMap<String, Flake>>,
    #[serde(default)]
    pub dev_shell: DevShell,
    #[serde(rename = "R")]
    pub r: Option<R>,
}

#[derive(Debug)]
pub struct TofuConfigToml {
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: TofuAnysnake2,
    pub nixpkgs: TofuNixpkgs,
    pub outside_nixpkgs: TofuVCS,
    pub ancient_poetry: TofuVCS,
    pub poetry2nix: TofuVCS,
    pub flake_util: TofuVCS,
    pub clone_regexps: Option<HashMap<String, String>>,
    pub clones: Option<HashMap<String, HashMap<String, TofuVCS>>>,
    pub cmd: HashMap<String, Cmd>,
    pub rust: Option<TofuRust>,
    pub python: Option<Python>,
    pub container: Container,
    pub flakes: HashMap<String, TofuFlake>,
    pub dev_shell: DevShell,
    pub r: Option<TofuR>,
}

//todo: refactor

#[derive(Debug, Deserialize)]
pub struct ParsedVCSInsideURLTag {
    pub url: ParsedVCS
}

impl ConfigToml {
    pub fn from_str(raw_config: &str) -> Result<ConfigToml> {
        let mut res: ConfigToml = toml::from_str(raw_config)?;
        Ok(res)
    }
    pub fn from_file(config_file: &str) -> Result<ConfigToml> {
        use ex::fs;
        let abs_config_path =
            fs::canonicalize(config_file).context("Could not find config file")?;
        let raw_config =
            fs::read_to_string(&abs_config_path).context("Could not read config file")?;
        let mut parsed_config: ConfigToml = Self::from_str(&raw_config).with_context(|| {
            crate::ErrorWithExitCode::new(65, format!("Failure parsing {:?}", &abs_config_path))
        })?;
        parsed_config.anysnake2_toml_path = Some(abs_config_path);
        Ok(parsed_config)
    }
}

impl MinimalConfigToml {
    pub fn from_str(raw_config: &str) -> Result<MinimalConfigToml> {
        let mut res: MinimalConfigToml = toml::from_str(raw_config)?;
        Ok(res)
    }

    pub fn from_file(config_file: &str) -> Result<MinimalConfigToml> {
        use ex::fs;
        let abs_config_path =
            fs::canonicalize(config_file).context("Could not find config file")?;
        let raw_config =
            fs::read_to_string(&abs_config_path).context("Could not read config file")?;
        let mut parsed_config: MinimalConfigToml =
            Self::from_str(&raw_config).with_context(|| {
                crate::ErrorWithExitCode::new(65, format!("Failure parsing {:?}", &abs_config_path))
            })?;
        parsed_config.anysnake2_toml_path = Some(abs_config_path);
        Ok(parsed_config)
    }
}

impl<'de> Deserialize<'de> for ParsedVCS {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Use `serde::from_str` to attempt conversion from string slice to `MyType`
        //let s = String::deserialize(deserializer)?;
        //let map: HashMap<String, String> = HashMap::deserialize(deserializer)?;
        //let url = map.get("url");
        let url = <Option<String>>::deserialize(deserializer)?;
        match url {
            Some(s) => ParsedVCS::try_from(s.as_str()).map_err(serde::de::Error::custom),
            None => Err(serde::de::Error::custom("Expected url in field")),
        }
    }
}

#[derive(Debug)]
pub enum ParsedVCSorDev {
    VCS(ParsedVCS),
    Dev,
}

impl<'de> Deserialize<'de> for ParsedVCSorDev {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Use `serde::from_str` to attempt conversion from string slice to `MyType`
        let s = String::deserialize(deserializer)?;
        Ok(if s == "dev" {
            ParsedVCSorDev::Dev
        } else {
            ParsedVCSorDev::VCS(ParsedVCS::try_from(s.as_str()).map_err(serde::de::Error::custom)?)
        })
    }
}

#[derive(Debug)]
pub enum TofuVCSorDev {
    VCS(TofuVCS),
    Dev,
}

impl TryFrom<ParsedVCSorDev> for TofuVCSorDev {
    type Error = anyhow::Error;

    fn try_from(value: ParsedVCSorDev) -> std::prelude::v1::Result<Self, Self::Error> {
        match value {
            ParsedVCSorDev::VCS(v) => Ok(TofuVCSorDev::VCS(TofuVCS::try_from(v)?)),
            ParsedVCSorDev::Dev => Ok(TofuVCSorDev::Dev),
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct Anysnake2 {
    pub url: Option<ParsedVCSorDev>,
    #[serde(default = "Anysnake2::default_use_binary")]
    pub use_binary: bool,
    pub do_not_modify_flake: Option<bool>,
    #[serde(default = "Anysnake2::default_dtach")]
    pub dtach: bool,
}
#[derive(Debug)]
pub struct TofuAnysnake2 {
    pub url: TofuVCSorDev,
    pub use_binary: bool,
    pub do_not_modify_flake: bool,
    pub dtach: bool,
}

impl Anysnake2 {
    pub fn default_use_binary() -> bool {
        true
    }
    pub fn default_dtach() -> bool {
        true
    }
}

#[derive(Deserialize, Debug)]
pub struct DevShell {
    pub inputs: Option<Vec<String>>,
    #[serde(default = "DevShell::default_shell")]
    pub shell: String,
}

impl DevShell {
    fn default_shell() -> String {
        "bash".to_string()
    }
}

impl Default for DevShell {
    fn default() -> Self {
        DevShell {
            inputs: None,
            shell: Self::default_shell(),
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct NixPkgs {
    //tell serde to read it from url/rev instead
    pub url: Option<ParsedVCS>,
    pub packages: Option<Vec<String>>,
    #[serde(default = "NixPkgs::default_allow_unfree")]
    pub allow_unfree: bool,
}

impl NixPkgs {
    pub fn new() -> Self {
        NixPkgs {
            url: None,
            packages: None,
            allow_unfree: Self::default_allow_unfree(),
        }
    }
    pub fn default_allow_unfree() -> bool {
        false
    }
}

#[derive(Debug)]
pub struct TofuNixpkgs {
    pub url: TofuVCS,
    pub packages: Vec<String>,
    pub allow_unfree: bool,
}

#[derive(Deserialize, Debug)]
pub struct Cmd {
    pub run: String,
    pub pre_run_outside: Option<String>,
    pub while_run_outside: Option<String>,
    pub post_run_inside: Option<String>,
    pub post_run_outside: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Rust {
    pub version: Option<String>,
    pub url: Option<ParsedVCS>,
}

#[derive(Debug)]
pub struct TofuRust {
    pub version: Option<String>,
    pub url: TofuVCS,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ParsedPythonPackageDefinition {
    Simple(String),
    Complex(HashMap<String, toml::Value>),
}

#[derive(Debug, Clone)]
pub struct BuildPythonPackageInfo {
    options: HashMap<String, String>,
    pub overrides: Option<Vec<String>>,
}

impl BuildPythonPackageInfo {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.options.get(key)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.options.contains_key(key)
    }
    pub fn insert(&mut self, key: String, value: String) -> Option<String> {
        self.options.insert(key, value)
    }
    pub fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&String, &mut String) -> bool,
    {
        self.options.retain(f);
    }

    pub fn src_to_nix(&self) -> String {
        let mut res = Vec::new();
        let inherit_pname: bool = self
            .options
            .get("method")
            .map_or(false, |x| x == "fetchPypi");
        for (k, v) in self.options.iter().sorted_by_key(|x| x.0) {
            if k != "method" && k != "buildInputs" && k != "buildPythonPackage_arguments" {
                res.push(format!("\"{}\" = \"{}\";", k, v));
            }
        }
        if inherit_pname && !self.options.contains_key("pname") {
            res.push("inherit pname;".to_string());
        }
        res.join("\n")
    }
}

#[derive(Debug, Clone)]
pub enum PythonPackageDefinition {
    Simple(String),
    Editable(String),
    Complex(toml::map::Map<String, toml::Value>),
}

fn de_python_package_definition<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, PythonPackageDefinition>, D::Error>
where
    D: Deserializer<'de>,
{
    let parsed: HashMap<String, ParsedPythonPackageDefinition> =
        crate::maps_duplicate_key_is_error::deserialize(deserializer)?;
    let res: Result<HashMap<String, PythonPackageDefinition>, D::Error> = parsed
        .into_iter()
        .map(|(pkg_name, v)| {
            match v {
                ParsedPythonPackageDefinition::Simple(x) => {

                    Ok((pkg_name,
                        if x.starts_with("editable/") { PythonPackageDefinition::Editable(x.strip_prefix("editable/").unwrap().to_string())} else { PythonPackageDefinition::Simple(x) }))
                }
                ParsedPythonPackageDefinition::Complex(def) => {
                    let mut errors = Vec::new();
                    let mut parsed_def = toml::map::Map::new();
                    let allowed_keys =["url", "poetry2nix", "version", "git", "branchName", "rev", "pypi"];
                    let mut url_used = false;
                    let mut version_used = false;
                    for (key, value) in def.into_iter() {
                        match value {
                            toml::Value::String(_)| toml::Value::Array(_) | toml::Value::Table(_) => {
                                if key == "method" {
                                    errors.push("Method has been superseeded by fetchgit=git=<url>, rev=<rev>, fetchFromGitHub: git=<https://github.com...}=. fetchhg: no longer supported (sorry). See the examples".to_string());
                                    continue;
                                }
                                if key == "url" {
                                    url_used = true;
                                }
                                else if key == "version" {
                                    version_used = true;
                                }

                                if allowed_keys.contains(&key.as_str()) {
                                    parsed_def.insert(key, value);
                                } else {
                                    if key.starts_with("hash_") {
                                    }
                                    else if key.starts_with("pypi_url"){
                                        parsed_def.insert(key, value);
                                    }
                                    else {
                                        errors.push(format!("Unexpected key: {}", key));
                                    }
                                }
                            }
                            _ => {
                                errors.push("All python package definition values must be strings, or list of strings.".to_string());
                            }
                        }
                        if url_used && version_used {
                            errors.push("Both url and version are used, but only one is allowed.".to_string());
                        }
                    }
                    if errors.is_empty() {
                        Ok((
                            pkg_name,
                            PythonPackageDefinition::Complex(parsed_def)))
                    } else {
                        Err(serde::de::Error::custom(format!(
                            "Python.packages.{}: {}",
                            pkg_name,
                            errors.join("\n")
                        )))
                    }
                }


                    /* let method = def
                        .get("method")
                        .ok_or_else(|| {
                            serde::de::Error::custom(format!(
                                "Missing method on python package {}",
                                pkg_name
                            ))
                        })?
                        .as_str()
                        .ok_or_else(|| {
                            serde::de::Error::custom(format!(
                                "method must be a string on python package {}",
                                pkg_name
                            ))
                        })?;
                    match method {
                        "fetchFromGitHub" => {
                            if !def.contains_key("owner") {
                                errors.push("Was missing 'owner' key.")
                            }
                            if !def.contains_key("repo") {
                                errors.push("Was missing 'repo' key.")
                            }
                        }
                        "fetchGit" | "fetchhg" => {
                            if !def.contains_key("url") {
                                errors.push("Was missing 'url' key.")
                            }
                        }
                        _ => {}
                    }
                    if !errors.is_empty() {
                        return Err(serde::de::Error::custom(format!(
                            "Python.packages.{}: {}",
                            pkg_name,
                            errors.join("\n")
                        )));
                    }
                    let overrides = match def.get("overrides") {
                        None => None,
                        Some(toml::Value::Array(input)) => {
                            let mut output: Vec<String> = Vec::new();
                            for ov in input.iter() {
                                output.push(
                                    ov.as_str()
                                        .ok_or(serde::de::Error::custom(format!(
                                            "Overrides must be an array of strings. Python package {}",
                                            pkg_name,
                                        )))?
                                        .to_string(),
                                );
                            }
                            Some(output)
                        }
                        Some(_) => {
                            return Err(serde::de::Error::custom(format!(
                                "Overrides must be an array of strings. Python package {}",
                                pkg_name,
                            )));
                        }
                    };
                    let string_defs: Result<HashMap<String, String>, D::Error> = def
                        .into_iter()
                        .filter_map(|(k, v)| match v {
                            toml::Value::String(v) => {
                                if k == "pkg_option" {
                                    let v = v.trim();
                                    if ! (v.starts_with('{') && v.ends_with('}')) {
                                        return Some(Err(serde::de::Error::custom(format!(
                                            "Field {} on python package {} must be the string representwation of the nix attrSet that we shall pass to buildPythonPackage",
                                            k, pkg_name
                                        ))));
                                    }
                                }

                                Some(Ok((k, v)))

                            },
                            toml::Value::Array(_) => {
                                if k != "overrides" {
                                    Some(Err(serde::de::Error::custom(format!(
                                        "Field {} on python package {} must be a string ",
                                        k, pkg_name
                                    ))))
                                } else {
                                    None
                                }
                            }
                            _ => {
                                Some(Err(serde::de::Error::custom(format!(
                                    "Field {} on python package {} must be a string ",
                                    k, pkg_name
                                ))))
                            }
                        })
                        .collect(); */
            }})
        .collect();
    res
}

#[derive(Deserialize, Debug)]
pub struct Python {
    pub version: String,
    pub ecosystem_date: String,
    #[serde(deserialize_with = "de_python_package_definition")]
    pub packages: HashMap<String, PythonPackageDefinition>,
    //pub additional_mkpython_arguments: Option<String>,
    //pub additional_mkpython_arguments_func: Option<String>,
}

impl Python {
    pub fn parsed_ecosystem_date(&self) -> Result<chrono::NaiveDate> {
        parse_my_date(&self.ecosystem_date)
    }
}

#[derive(Deserialize, Debug)]
pub struct Flake {
    pub url: ParsedVCS,
    pub follows: Option<Vec<String>>,
    pub packages: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct TofuFlake {
    pub url: TofuVCS,
    pub follows: Option<Vec<String>>,
    pub packages: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
pub struct Container {
    pub home: Option<String>,
    pub volumes_ro: Option<HashMap<String, String>>,
    pub volumes_rw: Option<HashMap<String, String>>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Debug)]
pub struct R {
    pub date: String,
    pub packages: Vec<String>,
    pub url: Option<ParsedVCS>,

    pub override_attrs: Option<HashMap<String, String>>,
    pub dependency_overrides: Option<HashMap<String, String>>,
    pub additional_packages: Option<HashMap<String, String>>,
}

#[derive(Debug)]
pub struct TofuR {
    pub date: String,
    pub packages: Vec<String>,
    pub url: TofuVCS,

    pub override_attrs: Option<HashMap<String, String>>,
    pub dependency_overrides: Option<HashMap<String, String>>,
    pub additional_packages: Option<HashMap<String, String>>,
}

fn parse_my_date(s: &str) -> Result<chrono::NaiveDate> {
    const FORMAT: &str = "%Y-%m-%d %H:%M:%S";
    Ok(
        chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00:00", s), FORMAT)?
            .and_utc()
            .date_naive(),
    )
}

impl ConfigToml {
    pub fn get_root_path_str(&self) -> Result<String> {
        let abs_config_path = self
            .anysnake2_toml_path
            .as_ref()
            .context("Config path not set???")?;
        let root = abs_config_path
            .parent()
            .context("config file had no parent path")?;
        Ok(root
            .to_owned()
            .into_os_string()
            .to_string_lossy()
            .to_string())
    }
}

impl TofuConfigToml {
    pub fn get_root_path_str(&self) -> Result<String> {
        let abs_config_path = self
            .anysnake2_toml_path
            .as_ref()
            .context("Config path not set???")?;
        let root = abs_config_path
            .parent()
            .context("config file had no parent path")?;
        Ok(root
            .to_owned()
            .into_os_string()
            .to_string_lossy()
            .to_string())
    }
}
