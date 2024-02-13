use anyhow::{Context, Result};
use log::info;
use toml_edit::Document;

use std::path::{Path, PathBuf};

/// A trait for converting a path to a string, with a lossy conversion.
pub trait CloneStringLossy {
    fn to_string_lossy(&self) -> String;
}

impl CloneStringLossy for PathBuf {
    fn to_string_lossy(&self) -> String {
        self.clone().into_os_string().to_string_lossy().to_string()
    }
}
impl CloneStringLossy for Path {
    fn to_string_lossy(&self) -> String {
        self.to_owned()
            .into_os_string()
            .to_string_lossy()
            .to_string()
    }
}

/// Split a string into lines, and add line numbers (4 digits)
/// before each line.
pub fn add_line_numbers(s: &str) -> String {
    let mut out = String::new();
    for (i, line) in s.lines().enumerate() {
        out.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }
    out
}

pub fn dir_empty(path: &Path) -> Result<bool> {
    Ok(path
        .read_dir()
        .context("Failed to read_dir")?
        .next()
        .is_none())
}

pub type TomlUpdates = Vec<(Vec<String>, toml_edit::Value)>;
/// if no hash_{rev} is set, discover it and update anysnake2.toml

pub fn change_toml_file(
    toml_path: &PathBuf,
    mod_func: impl FnOnce(&Document) -> Result<TomlUpdates>,
) -> Result<()> {
    let toml = std::fs::read_to_string(toml_path).expect("Could not reread config file");
    let mut doc = toml.parse::<Document>().expect("invalid doc");
    let updates = mod_func(&doc)?;
    if !updates.is_empty() {
        for (path, value) in updates {
            let mut x = &mut doc[&path[0]];
            for p in path[1..path.len() - 1].iter() {
                x = &mut x[p];
            }
            x[&path[path.len() - 1]] = toml_edit::Item::Value(value);
        }

        let out_toml = doc.to_string();
        std::fs::write(toml_path, out_toml).expect("failed to rewrite config file");
        info!("Wrote updated {:?}", toml_path);
    }

    Ok(())
}
