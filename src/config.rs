use anyhow::{Context, Result};
use serde_derive::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

trait WithDefaultFlakeSource {
    fn default_rev() -> String;
    fn default_url() -> String;
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

impl ConfigToml {
    pub fn from_str(raw_config: &str) -> Result<ConfigToml> {
        let mut res: ConfigToml = toml::from_str(&raw_config)?;
        res.anysnake2.url = match res.anysnake2.url {
            Some(url) => Some(url),
            None => match res.anysnake2.use_binary {
                true => Some("github:TyberiusPrime/anysnake2_release_flakes".to_string()),
                false => Some("github:TyberiusPrime/anysnake2".to_string()),
            },
        };
        Ok(res)
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
}
impl NixPkgs {
    fn default_url() -> String {
        "github:NixOS/nixpkgs".to_string()
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
pub struct Python {
    pub version: String,
    pub ecosystem_date: String,
    #[serde(with = "crate::maps_duplicate_key_is_error")]
    pub packages: HashMap<String, String>,
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
        "31b21203a1350bff7c541e9dfdd4e07f76d874be".to_string() // 3.3.0 does not support overwritting py-deps-db
    }
    fn default_url() -> String {
        "github:DavHau/mach-nix".to_string()
    }
}

#[derive(Deserialize, Debug)]
pub struct Flake {
    pub url: String,
    pub rev: String,
    pub follows: Option<Vec<String>>,
    pub packages: Vec<String>,
}
#[derive(Deserialize, Debug)]
pub struct Container {
    pub home: Option<String>,
    pub volumes_ro: Option<HashMap<String, String>>,
    pub volumes_rw: Option<HashMap<String, String>>,
    pub env: Option<HashMap<String, String>>,
}

impl Default for Container {
    fn default() -> Self {
        Container {
            home: None,
            volumes_ro: None,
            volumes_rw: None,
            env: None,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct R {
    pub ecosystem_tag: String,
    pub packages: Vec<String>,
    #[serde(default = "R::default_url")]
    pub r_ecosystem_track_url: String,
}

impl R {
    fn default_url() -> String{
        "github:TyberiusPrime/r_ecosystem_track".to_string()
    }
}

fn parse_my_date(s: &str) -> Result<chrono::NaiveDate> {
    const FORMAT: &str = "%Y-%m-%d %H:%M:%S";
    use chrono::TimeZone;
    Ok(chrono::Utc
        .datetime_from_str(&format!("{} 00:00:00", s), FORMAT)?
        .naive_utc()
        .date())
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
