use anyhow::{Result, bail};
use std::path::Path;

//
// parse a python egg file
pub fn parse_egg(egg_link: impl AsRef<Path>) -> Result<String> {
    let raw = ex::fs::read_to_string(egg_link)?;
    Ok(match raw.split_once('\n') {
        Some(x) => x.0.to_string(),
        None => raw,
    })
}


pub fn parse_pth_for_import(pth_link: impl AsRef<Path>) -> Result<String> {
    let raw = ex::fs::read_to_string(pth_link)?;
    if !raw.starts_with("import ") {
        bail!("pth did not start with import - unexpected");
    }
    let editable = raw.strip_prefix("import ").unwrap();
    let (module_name, _) = editable.split_once(';').unwrap();
    Ok(format!("{module_name}.py"))
}
//parse a python pth
