use crate::vcs::{ParsedVCS, TofuVCS};
use anyhow::{Context, Result};

#[allow(unused_imports)]
use log::debug;
use serde::de::Deserializer;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::prelude::v1::Result as StdResult;

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
#[allow(clippy::module_name_repetitions)]
pub struct ConfigToml {
    #[serde(skip)]
    pub anysnake2_toml_path: Option<PathBuf>,
    pub anysnake2: Anysnake2,
    pub nixpkgs: Option<NixPkgs>,
    pub outside_nixpkgs: Option<ParsedVCSInsideURLTag>,
    pub ancient_poetry: Option<ParsedVCSInsideURLTag>,
    pub poetry2nix: Option<Poetry2Nix>,
    #[serde(default, rename = "flake-util")]
    pub flake_util: Option<ParsedVCSInsideURLTag>,
    pub clone_regexps: Option<HashMap<String, String>>,
    pub clones: Option<HashMap<String, HashMap<String, String>>>,
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
    pub poetry2nix: TofuPoetry2Nix,
    pub flake_util: TofuVCS,
    pub clone_regexps: Option<Vec<(regex::Regex, String)>>,
    pub clones: Option<HashMap<String, HashMap<String, TofuVCS>>>,
    pub cmd: HashMap<String, Cmd>,
    pub rust: Option<TofuRust>,
    pub python: Option<TofuPython>,
    pub container: Container,
    pub flakes: HashMap<String, TofuFlake>,
    pub dev_shell: DevShell,
    pub r: Option<TofuR>,
}

//todo: refactor
#[derive(Debug, Deserialize)]
pub struct ParsedVCSInsideURLTag {
    pub url: Option<ParsedVCS>,
}

#[derive(Debug, Deserialize)]
pub struct Poetry2Nix {
    pub url: Option<ParsedVCS>,
    pub prefer_wheels: Option<bool>,
}

#[derive(Debug)]
pub struct TofuPoetry2Nix {
    pub source: TofuVCS,
    pub prefer_wheels: bool,
}

impl ConfigToml {
    pub fn from_str(raw_config: &str) -> Result<ConfigToml> {
        let res: ConfigToml = toml::from_str(raw_config)?;
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
        let res: MinimalConfigToml = toml::from_str(raw_config)?;
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
        let url = <Option<String>>::deserialize(deserializer)?;
        match url {
            Some(s) => ParsedVCS::try_from(s.as_str()).map_err(serde::de::Error::custom),
            None => Err(serde::de::Error::custom("Expected url in field")),
        }
    }
}

#[derive(Debug)]
pub enum ParsedVCSorDev {
    Vcs(ParsedVCS),
    Dev,
}

impl<'de> Deserialize<'de> for ParsedVCSorDev {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Use `serde::from_str` to attempt conversion from string slice to `MyType`
        let parsed = String::deserialize(deserializer)?;
        Ok(if parsed == "dev" {
            ParsedVCSorDev::Dev
        } else {
            ParsedVCSorDev::Vcs(
                ParsedVCS::try_from(parsed.as_str()).map_err(serde::de::Error::custom)?,
            )
        })
    }
}

#[derive(Debug)]
pub enum TofuVCSorDev {
    Vcs(TofuVCS),
    Dev,
}

impl TryFrom<ParsedVCSorDev> for TofuVCSorDev {
    type Error = anyhow::Error;

    fn try_from(value: ParsedVCSorDev) -> std::prelude::v1::Result<Self, Self::Error> {
        match value {
            ParsedVCSorDev::Vcs(v) => Ok(TofuVCSorDev::Vcs(TofuVCS::try_from(v)?)),
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

/* #[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ParsedPythonPackageDefinition {
    Simple(String),
    Complex(HashMap<String, toml::Value>),
} */

#[derive(Debug, Clone)]
pub struct BuildPythonPackageInfo {
    pub overrides: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum PythonPackageSource {
    VersionConstraint(String),
    Url(String),
    Vcs(ParsedVCS),
    PyPi { version: Option<String> },
}

#[derive(Debug, Clone)]
pub enum TofuPythonPackageSource {
    VersionConstraint(String),
    Url(String),
    Vcs(TofuVCS),
    PyPi { version: String },
}

impl PythonPackageSource {
    fn from_url(url: &str) -> Result<PythonPackageSource> {
        Ok(
            if url.starts_with("github:")
                | url.starts_with("git+https")
                | url.starts_with("hg+https:/")
            {
                let vcs = ParsedVCS::try_from(url)?;
                PythonPackageSource::Vcs(vcs)
            } else {
                PythonPackageSource::Url(url.to_string())
            },
        )
    }
}

#[derive(Debug, Clone)]
pub struct PythonPackageDefinition {
    pub source: PythonPackageSource,
    pub editable_path: Option<String>,
    pub poetry2nix: toml::map::Map<String, toml::Value>,
}

#[derive(Debug, Clone)]
pub struct TofuPythonPackageDefinition {
    pub source: TofuPythonPackageSource,
    pub editable_path: Option<String>,
    pub poetry2nix: toml::map::Map<String, toml::Value>,
}

#[derive(Debug)]
enum StrOrHashMap {
    String(String),
    HashMap(HashMap<String, toml::Value>),
}

impl<'de> Deserialize<'de> for StrOrHashMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrOrHashMapVisitor;

        impl<'de> serde::de::Visitor<'de> for StrOrHashMapVisitor {
            type Value = StrOrHashMap;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "a string or a hashmap")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(StrOrHashMap::String(v.to_string()))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut values = HashMap::new();
                while let Some((key, value)) = map.next_entry()? {
                    values.insert(key, value);
                }
                Ok(StrOrHashMap::HashMap(values))
            }
        }

        deserializer.deserialize_any(StrOrHashMapVisitor)
    }
}

impl<'de> Deserialize<'de> for PythonPackageDefinition {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let parsed = StrOrHashMap::deserialize(deserializer)?;
        match parsed {
            StrOrHashMap::String(str) => {
                let source = if str.contains(':') {
                    PythonPackageSource::from_url(str.as_str()).map_err(serde::de::Error::custom)?
                } else {
                    PythonPackageSource::VersionConstraint(str)
                };
                Ok(PythonPackageDefinition {
                    source,
                    editable_path: None,
                    poetry2nix: toml::map::Map::new(),
                })
            }
            StrOrHashMap::HashMap(parsed) => {
                if parsed.contains_key("preferWheel") {
                    return Err(serde::de::Error::custom(
                        "preferWheel is not a valid key, did you mean poetry2nix.preferWheel?",
                    ));
                }
                if parsed.contains_key("buildInputs") {
                    return Err(serde::de::Error::custom(
                        "preferWheel is not a valid key, did you mean poetry2nix.buildInputs?",
                    ));
                }
                let allowed_keys = &["url", "version", "poetry2nix", "editable"];
                for key in &parsed {
                    if !allowed_keys.contains(&key.0.as_str()) {
                        return Err(serde::de::Error::custom(format!(
                            "Invalid key {key:?} in package definition",
                        )));
                    }
                }
                let url = parsed.get("url");
                let version = parsed.get("version");

                if url.is_some() && version.is_some() {
                    return Err(serde::de::Error::custom(
                        "Both url and version are used, but only one is allowed. ",
                    ));
                }
                let source = {
                    if let Some(toml::Value::String(url)) = url {
                        PythonPackageSource::from_url(url.as_str())
                            .map_err(serde::de::Error::custom)?
                    } else if let Some(url) = url {
                        return Err(serde::de::Error::custom(format!(
                            "url must be a string, but was {url:?}",
                        )));
                    } else if let Some(toml::Value::String(constraint)) = version {
                        if constraint.starts_with("pypi:") {
                            PythonPackageSource::PyPi {
                                version: Some(constraint.split_once(':').unwrap().1.to_string()),
                            }
                        } else if constraint == "pypi" {
                            PythonPackageSource::PyPi { version: Some("".to_string()) }
                        } else {
                            PythonPackageSource::VersionConstraint(constraint.to_string())
                        }
                    } else if let Some(constraint) = version {
                        return Err(serde::de::Error::custom(format!(
                            "version must be a string, but was {constraint:?}",
                        )));
                    } else {
                        // this is a case of 'it's only here for poetry2nix.* or such
                        PythonPackageSource::VersionConstraint(String::default())
                    }
                };
                let editable = {
                    let str_val = parsed.get("editable").and_then(toml::Value::as_str);
                    if let Some(str_val) = str_val {
                        Some(str_val.to_string())
                    } else {
                        let b = parsed.get("editable").and_then(toml::Value::as_bool);
                        match b {
                            Some(true) => Some("code".to_string()),
                            _ => None,
                        }
                    }
                };
                let poetry2nix = match parsed.get("poetry2nix") {
                    Some(entry) => Ok(entry
                        .as_table()
                        .cloned()
                        .context("poetry2nix was not a table")
                        .map_err(serde::de::Error::custom)?),
                    None => Ok(toml::map::Map::new()),
                }?;
                Ok(PythonPackageDefinition {
                    source,
                    editable_path: editable,
                    poetry2nix,
                })
            }
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct Python {
    pub version: String,
    pub ecosystem_date: String,
    pub packages: HashMap<String, PythonPackageDefinition>,
}

#[derive(Debug)]
pub struct TofuPython {
    pub version: String,
    pub ecosystem_date: String,
    pub packages: HashMap<String, TofuPythonPackageDefinition>,
}

impl TofuPython {
    pub fn parsed_ecosystem_date(&self) -> Result<chrono::NaiveDate> {
        parse_my_date(&self.ecosystem_date)
    }

    pub fn has_editable_packages(&self) -> bool {
        for spec in self.packages.values() {
            if spec.editable_path.is_some() {
                return true;
            }
        }
        false
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

fn parse_my_date(input: &str) -> Result<chrono::NaiveDate> {
    const FORMAT: &str = "%Y-%m-%d %H:%M:%S";
    Ok(
        chrono::NaiveDateTime::parse_from_str(&format!("{input} 00:00:00"), FORMAT)?
            .and_utc()
            .date_naive(),
    )
}
