use anyhow::Result;
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
