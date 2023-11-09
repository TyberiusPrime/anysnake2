use anyhow::{Context, Result};
use itertools::Itertools;
use serde::de::Deserializer;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

trait WithDefaultFlakeSource {
    fn default_rev() -> String;
    fn default_url() -> String;
}

//just enough to read the requested version
#[derive(Deserialize, Debug)]
pub struct MinimalConfigToml {
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: Anysnake2,
}

#[derive(Deserialize, Debug)]
pub struct ConfigToml {
    #[serde(skip)]
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: Anysnake2,
    pub nixpkgs: NixPkgs,
    pub outside_nixpkgs: NixPkgs,
    #[serde(default, rename = "flake-util")]
    pub flake_util: FlakeUtil,
    pub clone_regexps: Option<HashMap<String, String>>,
    pub clones: Option<HashMap<String, HashMap<String, String>>>,
    #[serde(default)]
    pub cmd: HashMap<String, Cmd>,
    #[serde(default)]
    pub rust: Rust,
    pub python: Option<Python>,
    #[serde(default, rename = "mach-nix")]
    pub mach_nix: MachNix,
    #[serde(default)]
    pub container: Container,
    pub flakes: Option<HashMap<String, Flake>>,
    #[serde(default)]
    pub dev_shell: DevShell,
    #[serde(rename = "R")]
    pub r: Option<R>,
}

//todo: refactor

impl ConfigToml {
    pub fn from_str(raw_config: &str) -> Result<ConfigToml> {
        let mut res: ConfigToml = toml::from_str(raw_config)?;
        res.anysnake2.url = match res.anysnake2.url {
            Some(url) => Some(url),
            None => match res.anysnake2.use_binary {
                true => Some("github:TyberiusPrime/anysnake2_release_flakes".to_string()),
                false => Some("github:TyberiusPrime/anysnake2".to_string()),
            },
        };
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
        res.anysnake2.url = match res.anysnake2.url {
            Some(url) => Some(url),
            None => match res.anysnake2.use_binary {
                true => Some("github:TyberiusPrime/anysnake2_release_flakes".to_string()),
                false => Some("github:TyberiusPrime/anysnake2".to_string()),
            },
        };
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

#[derive(Deserialize, Debug)]
pub struct Anysnake2 {
    pub rev: String,
    #[serde(default = "Anysnake2::default_use_binary")]
    pub use_binary: bool,
    pub url: Option<String>,
    pub do_not_modify_flake: Option<bool>,
    #[serde(default = "Anysnake2::default_dtach")]
    pub dtach: bool,
}

impl Anysnake2 {
    fn default_use_binary() -> bool {
        true
    }
    fn default_dtach() -> bool {
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
    pub rev: String,
    #[serde(default = "NixPkgs::default_url")]
    pub url: String,
    pub packages: Option<Vec<String>>,
    #[serde(default = "NixPkgs::default_allow_unfree")]
    pub allow_unfree: bool,
}

impl NixPkgs {
    fn default_url() -> String {
        "github:NixOS/nixpkgs".to_string()
    }
    fn default_allow_unfree() -> bool {
        false
    }
}

#[derive(Deserialize, Debug)]
pub struct FlakeUtil {
    #[serde(default = "FlakeUtil::default_rev")]
    pub rev: String,
    #[serde(default = "FlakeUtil::default_url")]
    pub url: String,
}

impl WithDefaultFlakeSource for FlakeUtil {
    fn default_rev() -> String {
        "7e5bf3925f6fbdfaf50a2a7ca0be2879c4261d19".to_string()
    }

    fn default_url() -> String {
        "github:numtide/flake-utils".to_string()
    }
}

impl Default for FlakeUtil {
    fn default() -> Self {
        FlakeUtil {
            rev: Self::default_rev(),
            url: Self::default_url(),
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct Cmd {
    pub run: String,
    pub pre_run_outside: Option<String>,
    pub post_run_inside: Option<String>,
    pub post_run_outside: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Rust {
    pub version: Option<String>,
    #[serde(default = "Rust::default_rev")]
    pub rust_overlay_rev: String,
    #[serde(default = "Rust::default_url")]
    pub rust_overlay_url: String,
}

impl Default for Rust {
    fn default() -> Self {
        Rust {
            version: None,
            rust_overlay_rev: Self::default_rev(),
            rust_overlay_url: Self::default_url(),
        }
    }
}

impl WithDefaultFlakeSource for Rust {
    fn default_rev() -> String {
        "08de2ff90cc08e7f9523ad97e4c1653b09f703ec".to_string()
    }
    fn default_url() -> String {
        "github:oxalica/rust-overlay".to_string()
    }
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ParsedPythonPackageDefinition {
    Requirement(String),
    BuildPythonPackage(HashMap<String, toml::Value>),
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
        let inherit_pname: bool = self.options.get("method").map_or(false, |x| x == "fetchPypi");
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
    Requirement(String),
    BuildPythonPackage(BuildPythonPackageInfo),
}

//todo: trust on first use, or at least complain if never seen before rev and
//mismatch on sha:
//nix-prefetch-url https://github.com/TyberiusPrime/dppd/archive/b55ac32ef322a8edfc7fa1b6e4553f66da26a156.tar.gz --type sha256 --unpack
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
        .map(|(pkg_name, v)| match v {
            ParsedPythonPackageDefinition::Requirement(x) => {
                Ok((pkg_name, PythonPackageDefinition::Requirement(x)))
            }
            ParsedPythonPackageDefinition::BuildPythonPackage(def) => {
                let mut errors: Vec<&str> = Vec::new();
                let method = def
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
                    .collect();
                Ok((
                    pkg_name,
                    PythonPackageDefinition::BuildPythonPackage(BuildPythonPackageInfo {
                        options: string_defs?,
                        overrides,
                    }),
                ))
            }
        })
        .collect();
    res
}

#[derive(Deserialize, Debug)]
pub struct Python {
    pub version: String,
    pub ecosystem_date: String,
    #[serde(deserialize_with = "de_python_package_definition")]
    pub packages: HashMap<String, PythonPackageDefinition>,
    pub additional_mkpython_arguments: Option<String>,
}

impl Python {
    pub fn parsed_ecosystem_date(&self) -> Result<chrono::NaiveDate> {
        parse_my_date(&self.ecosystem_date)
    }
}
#[derive(Deserialize, Debug)]
pub struct MachNix {
    #[serde(default = "MachNix::default_rev")]
    pub rev: String,
    #[serde(default = "MachNix::default_url")]
    pub url: String,
}

impl Default for MachNix {
    fn default() -> Self {
        MachNix {
            rev: Self::default_rev(),
            url: Self::default_url(),
        }
    }
}

impl WithDefaultFlakeSource for MachNix {
    fn default_rev() -> String {
        "65266b5cc867fec2cb6a25409dd7cd12251f6107".to_string() //updated 2022-12-02
                                                               //"7e14360bde07dcae32e5e24f366c83272f52923f".to_string() // updated 2022-07-11
                                                               // "bdc97ba6b2ecd045a467b008cff4ae337b6a7a6b".to_string() // updated 2022-24-01
    }
    fn default_url() -> String {
        "github:DavHau/mach-nix".to_string()
    }
}

#[derive(Deserialize, Debug)]
pub struct Flake {
    pub url: String,
    pub rev: Option<String>,
    pub follows: Option<Vec<String>>,
    pub packages: Option<Vec<String>>,
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
    #[serde(default = "R::default_url")]
    pub nixr_url: String,
    #[serde(default = "R::default_tag")]
    pub nixr_tag: String,

    pub ecosystem_tag: Option<String>,

    pub override_attrs: Option<HashMap<String, String>>,
}

impl R {
    fn default_tag() -> String {
        //yes, this is the nixR version
        "f77d23b8b6ec1c7009d8251edd60590517a54bbf".to_string() // must not be older than 5fa155779f1c454fcb92abcdbbcf4372256eb6c6
    }

    fn default_url() -> String {
        "github:TyberiusPrime/nixR".to_string()
    }
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
