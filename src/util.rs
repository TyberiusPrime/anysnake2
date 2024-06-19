use anyhow::{Context, Result};
#[allow(unused_imports)]
use log::{debug, info};
use toml_edit::DocumentMut;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

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

pub type TomlUpdates = Vec<(Vec<String>, toml_edit::Item)>;

fn apply_table_order(document: &mut DocumentMut, order: &HashMap<&str, usize>) {
    for (k, v) in document.iter_mut() {
        if let toml_edit::Item::Table(t) = v {
            let position = order
                .get(k.get())
                .map_or_else(|| k.chars().next().unwrap() as usize * 255, |x| *x);
            t.set_position(position);

            for (k2, v2) in t.iter_mut() {
                let k2k = format!("{}.{}", k.get(), k2.get());
                if let toml_edit::Item::Table(t2) = v2 {
                    let position2 = order.get(k2k.as_str()).map_or_else(
                        || (position + (k2.get().chars().next().unwrap() as usize)),
                        |x| *x,
                    );
                    t2.set_position(position2);
                }
            }
        }
    }
}

pub fn change_toml_file(toml_path: &PathBuf, updates: TomlUpdates) -> Result<()> {
    let toml = std::fs::read_to_string(toml_path).expect("Could not reread config file");
    let mut doc = toml.parse::<DocumentMut>().expect("invalid doc");
    if !updates.is_empty() {
        debug!("Applying updates to {:?}", toml_path);
        debug!("{:?}", updates);
        for (path, value) in updates {
            if !doc.contains_key(&path[0]) {
                doc[&path[0]] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let mut x = &mut doc[&path[0]];
            if path.len() > 1 {
                for p in &path[1..path.len()] {
                    if let toml_edit::Item::Value(_t) = x { // if it was previously a value...
                        *x = toml_edit::Item::Value(
                            toml_edit::Table::new().into_inline_table().into(),
                        );
                    }
                    x = &mut x[p];
                }
                *x = value;
            } else {
                *x = value;
            }
        }

        let order: HashMap<&str, usize> = [
            ("anysnake2", 0),
            ("outside_nixpkgs", 1),
            ("ancient_poetry", 2),
            ("poetry2nix", 2),
            ("nixpkgs", 3),
            ("clones", 4),
            ("python", 10),
            ("python.packages", 11),
            ("R", 12),
            ("rust", 13),
            ("flakes", 14),
            ("dev_shell", 20),
            ("container", 21),
            ("env", 22),
            ("cmd", 23),
            ("flake_util", 99),
        ]
        .into_iter()
        .collect();
        apply_table_order(&mut doc, &order);

        let out_toml = doc.to_string();
        std::fs::write(toml_path, out_toml).context("failed to rewrite config file")?;
        info!("Wrote updated {:?}", toml_path);
    }

    Ok(())
}

/// retrieve an url, possibly using the http proxy from the environment
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
        agent = agent.proxy(proxy);
    }
    Ok(agent.build())
}
