use anyhow::{bail, Context, Result};
#[allow(unused_imports)]
use log::{debug, info};
use toml_edit::{DocumentMut, Item, KeyMut};

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

pub type TomlUpdates = Vec<(Vec<String>, toml_edit::Item)>;

fn assign_entry_prefix(key: &mut KeyMut) {
    {
        let ld = key.leaf_decor_mut();
        match ld.prefix() {
            Some(prefix) => match prefix.as_str() {
                Some(prefix) => {
                    let mut lines: Vec<_> =
                        prefix.split('\n').map(|x| x.trim().to_string()).collect();
                    if lines.is_empty() {
                        lines.push("\t".into());
                    } else {
                        if lines.len() > 1 && lines.first().unwrap().is_empty() {
                            lines.remove(0);
                        }
                        lines.iter_mut().last().unwrap().push('\t');
                    }
                    ld.set_prefix(lines.join("\n"))
                }
                None => ld.set_prefix("\t"),
            },
            None => {
                //debug!("{}", key);
                ld.set_prefix("\t");
            }
        }
    }
}

fn descend_assign_scores(tbl: &mut toml_edit::Table, score: usize) {
    for mut el in tbl.iter_mut() {
        match el.1 {
            Item::Table(x) => {
                x.set_position(score);
                descend_assign_scores(x, score + 1);
                x.sort_values();
                let prefix = x
                    .decor()
                    .prefix()
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim_start()
                    .to_string();
                if prefix.is_empty() {
                    x.decor_mut().set_prefix("\n\n");
                } else {
                    x.decor_mut().set_prefix(format!("\n\n{prefix}"));
                }
            }
            Item::Value(v) => match v {
                toml_edit::Value::Array(arr) => {
                    assign_entry_prefix(&mut el.0);
                    if arr.len() > 1 {
                        for value in arr.iter_mut() {
                            let mut old = value
                                .decor()
                                .prefix()
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            if old.starts_with('#') {
                                old.insert(0, ' ');
                            }
                            value.decor_mut().set_prefix(format!("{old}\n\t\t"));
                        }
                    }
                }
                _ => {
                    assign_entry_prefix(&mut el.0);
                }
            },
            _ => {}
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
                    if let toml_edit::Item::Value(v) = x {
                        match v {
                            toml_edit::Value::InlineTable(_) => {}
                            _ => {
                                // if it was previously a value...
                                *x = toml_edit::Item::Value(
                                    toml_edit::Table::new().into_inline_table().into(),
                                );
                            }
                        }
                    }
                    x = &mut x[p];
                }
                *x = value;
            } else {
                *x = value;
            }
        }
    }

    //doc["nixpkgs"].as_table_mut().unwrap().sort_values_by(comp);
    for el in doc.iter_mut() {
        match el.1 {
            toml_edit::Item::Table(x) => {
                let score = get_score(el.0.get());
                x.set_position(score);
                descend_assign_scores(x, score + 1);
                x.sort_values();
                let prefix = x
                    .decor()
                    .prefix()
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim_start()
                    .to_string();
                if prefix.is_empty() {
                    x.decor_mut().set_prefix("\n\n");
                } else {
                    x.decor_mut().set_prefix(format!("\n\n{prefix}"));
                }
            }
            _ => {
                unimplemented!("el {}, not a table, unexpected", el.0)
            }
        }
    }

    let out_toml = doc.to_string();
    std::fs::write(toml_path, out_toml.trim_start()).context("failed to rewrite config file")?;
    info!("Wrote updated {:?}", toml_path);

    Ok(())
}

const ORDER_SCORES: &[(&str, usize)] = &[
    ("anysnake2", 1000),
    ("nixpkgs", 3000),
    ("clones", 4000),
    ("clone_regexps", 5000),
    ("python", 10000),
    ("python.packages", 11000),
    ("R", 12000),
    ("rust", 13000),
    ("flakes", 14000),
    ("dev_shell", 20000),
    ("container", 21000),
    ("env", 22000),
    ("cmd", 23000),
    ("dev_shell", 25000),
    ("outside_nixpkgs", 96000),
    ("ancient_poetry", 97000),
    ("poetry2nix", 98000),
    ("flake-util", 99000),
];

fn get_score(key: &str) -> usize {
    let mut res = 10000000;
    for (k, v) in ORDER_SCORES {
        if key.starts_with(k) {
            res = *v;
        }
    }
    res
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

pub fn get_pypi_package_source_url(package_name: &str, pypi_version: &str) -> Result<String> {
    let json = get_proxy_req()?
        .get(&format!("https://pypi.org/pypi/{package_name}/json"))
        .call()?
        .into_string()?;
    let json: serde_json::Value = serde_json::from_str(&json)?;
    let files = json["releases"][&pypi_version]
        .as_array()
        .context("No releases found")?;
    for file in files {
        if file["packagetype"] == "sdist" {
            return Ok(file["url"].as_str().context("no url in json")?.to_string());
        }
    }
    bail!("Could not find a sdist release");
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    #[test]
    fn test_auto_formatting() {
        let input_filename = "tests/toml_reorder/in.toml";
        let output_filename = "tests/toml_reorder/out.toml";
        ex::fs::copy(input_filename, output_filename).unwrap();
        super::change_toml_file(&PathBuf::from(output_filename), vec![]).unwrap();
        let actual = ex::fs::read_to_string(output_filename).unwrap();
        let should = ex::fs::read_to_string("tests/toml_reorder/should.toml").unwrap();
        assert_eq!(actual, should);
        ex::fs::remove_file(output_filename).unwrap();
    }
}
