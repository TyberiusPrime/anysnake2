use anyhow::{bail, Result};
use log::{debug, warn};
use std::collections::{HashSet};
use std::fs::File;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

// The output is wrapped in a Result to allow matching on errors
// Returns an Iterator to the Reader of the lines of the file.
fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}

pub fn find_python_requirements_for_editable(paths: &Vec<String>) -> Result<Vec<(String, String)>> {
    let mut res = HashSet::new();
    for target_dir in paths.iter() {
        debug!("Looking for python dependencies for {}", target_dir);
        let requirement_file: PathBuf = [target_dir, "requirements.txt"].iter().collect();
        if requirement_file.exists() {
            for line in read_lines(requirement_file)?
                .map(|line| line.unwrap_or_else(|_| "".to_string()))
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
            {
                res.insert(line);
            }
        }

        let setup_cfg_file: PathBuf = [target_dir, "setup.cfg"].iter().collect();
        debug!("looking for {:?}", &setup_cfg_file);
        if setup_cfg_file.exists() {
            let reqs = parse_python_config_file(&setup_cfg_file);
            match reqs {
                Err(e) => {
                    warn!("failed to parse {:?}: {}", setup_cfg_file, e)
                }
                Ok(mut reqs) => {
                    debug!("requirements {:?}", reqs);
                    for k in reqs.drain(..) {
                        res.insert(k); // identical lines!
                    }
                }
            };
        }
    }
    Ok(res.into_iter().map(parse_python_package_spec).collect())
}

fn parse_python_package_spec(spec_line: String) -> (String, String) {
    let pos = spec_line.find(&['>', '<', '=', '!'][..]);
    match pos {
        Some(pos) => {
            let (name, spec) = spec_line.split_at(pos);
            (name.to_string(), spec.to_string())
        }
        None => (spec_line, "".to_string()),
    }
}

fn parse_python_config_file(setup_cfg_file: &Path) -> Result<Vec<String>> {
    //configparser does not do multi line values
    //ini dies on them as well.
    //so we do our own poor man's parsing
    debug!("Parsing {:?}", &setup_cfg_file);
    let raw = std::fs::read_to_string(&setup_cfg_file)?;
    let mut res = Vec::new();
    match raw.find("[options]") {
        Some(options_start) => {
            let mut inside_value = false;
            let mut value_indention = 0;
            let mut value = "".to_string();
            for line in raw[options_start..].split('\n') {
                if !inside_value {
                    if line.contains("install_requires") {
                        let wo_indent_len = (line.replace("\t", "    ").trim_start()).len();
                        value_indention = line.len() - wo_indent_len;
                        match line.find('=') {
                            Some(equal_pos) => {
                                let v = line[equal_pos + 1..].trim_end();
                                value += v;
                                value += "\n";
                                inside_value = true;
                            }
                            None => bail!("No = in install_requires line"),
                        }
                    }
                } else {
                    // inside value
                    let wo_indent_len = (line.replace("\t", "    ").trim_start()).len();
                    let indent = line.len() - wo_indent_len;
                    if indent > value_indention {
                        value += line.trim_start();
                        value += "\n"
                    } else {
                        break;
                    }
                }
            }
            for line in value.split('\n') {
                if !line.trim().is_empty() {
                    res.push(line.trim().to_string())
                }
            }
        }
        None => bail!("no [options] in setup.cfg"),
    };
    Ok(res)
    //Err(anyhow!("Could not parse"))
}
