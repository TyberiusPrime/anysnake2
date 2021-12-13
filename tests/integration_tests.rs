use named_lock::NamedLock;
use std::path::PathBuf;
use std::process::Command;

fn run_test(cwd: &str, args: &[&str]) -> (i32, String, String) {
    //can't have more than one running from a given folder at a time
    //let lock = NamedLock::create(&cwd.replace("/", "_")).unwrap();
    let lock = NamedLock::create("anysnaketest").unwrap();
    let _guad = lock.lock().unwrap();
    //do not nuke flake dir, you'll overload the github rate limits quickly
    let flake_lock = PathBuf::from(cwd).join(".anysnake2_flake/flake.lock");
    if flake_lock.exists() {
        std::fs::remove_file(flake_lock).unwrap();
    }
    let result_dir = PathBuf::from(cwd).join(".anysnake2_flake/result");
    if result_dir.exists() {
        std::fs::remove_file(result_dir).unwrap();
    }



    let p = std::env::current_exe()
        .expect("No current exe?")
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("anysnake2");
    println!("Current exe {:?}", p);
    let mut full_args = vec!["--no-version-switch"];
    full_args.extend(args);
    let output = Command::new(p)
        .args(full_args)
        .current_dir(cwd)
        .env("TMUX", "true")
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    println!("code is: {}", code);
    println!("stdout is: {}", stdout);
    println!("stderr is: {}", stderr);
    (code, stdout.to_string(), stderr.to_string())
}

#[test]
fn test_minimal_no_python() {
    let (code, _stdout, stderr) =
        run_test("examples/minimal", &["run", "--", "python", "--version"]);
    assert!(code == 127);
    assert!(stderr.contains("python: command not found"));
}

#[test]
fn test_minimal_bash_version() {
    let (_code, stdout, _stderr) =
        run_test("examples/minimal", &["run", "--", "bash", "--version"]);
    assert!(stdout.contains("5.1.4(1)"));
}

#[test]
fn test_just_python() {
    let (_code, stdout, _stderr) = run_test(
        "examples/just_python",
        &["run", "--", "python", "--version"],
    );
    assert!(stdout.contains("3.8.9"));
}

#[test]
fn test_just_python_pandas_version() {
    let (_code, stdout, _stderr) = run_test(
        "examples/just_python",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import pandas; print(pandas.__version__)'",
        ],
    );
    assert!(stdout.contains("1.2.0"));
}

#[test]
fn test_no_anysnake_toml() {
    let (code, _stdout, stderr) = run_test(
        "examples/no_anysnake2_toml",
        &["run", "--", "python", "--version"],
    );
    assert!(code == 70);
    assert!(stderr.contains("anysnake2.toml"));
}

#[test]
fn test_basic() {
    let (_code, stdout, _stderr) = run_test("examples/basic", &["run", "--", "bash", "--version"]);
    assert!(stdout.contains("5.1.4"));
}

#[test]
fn test_basic_fish() {
    let (_code, stdout, _stderr) = run_test("examples/basic", &["run", "--", "fish", "--version"]);
    assert!(stdout.contains("3.2.2"));
}

#[test]
fn test_basic_python() {
    let (_code, stdout, _stderr) =
        run_test("examples/basic", &["run", "--", "python", "--version"]);
    assert!(stdout.contains("3.9.4"));
}

#[test]
fn test_basic_jupyter() {
    let (_code, stdout, _stderr) = run_test(
        "examples/basic",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import jupyter; print(jupyter.__version__)'",
        ],
    );
    assert!(stdout.contains("1.0.0"));
}

#[test]
fn test_basic_cargo() {
    let (_code, stdout, _stderr) = run_test("examples/basic", &["run", "--", "cargo", "--version"]);
    assert!(stdout.contains("1.55.0"));
}

#[test]
fn test_basic_projct_folder() {
    let (code, stdout, _stderr) = run_test(
        "examples/basic",
        &["run", "--", "ls", "/project/pandas_version.ipynb"],
    );
    assert!(stdout.contains("pandas_version.ipynb"));
    assert!(code == 0);
}

fn rm_clones(path: &str) {
    let pb = PathBuf::from(path).join("code");
    std::fs::remove_dir_all(pb).unwrap()
}

#[test]
fn test_full() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();

    rm_clones("examples/full");
    let (_code, stdout, _stderr) = run_test("examples/full", &["run", "--", "R", "--version"]);
    assert!(stdout.contains("4.1.1"));
    let out = Command::new("git")
        .args(&["log"])
        .current_dir("examples/full/code/dppd_plotnine")
        .output()
        .expect("git log failed");
    assert!(std::str::from_utf8(&out.stdout).unwrap().split('\n')
            .next().unwrap().contains("8ed7651af759f3f0b715a2fbda7bf5119f7145d7"))




}

#[test]
fn test_full_r_packages() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();

    rm_clones("examples/full");
    let (_code, stdout, _stderr) = run_test("examples/full", &["run", "--", "R", "-e", "'library(ACA);sessionInfo();'"]);
    assert!(stdout.contains("ACA_1.1"));
}


#[test]
fn test_full_hello() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();

    let (_code, stdout, _stderr) = run_test("examples/full", &["run", "--", "hello", "--version"]);
    assert!(stdout.contains("Hello World"));
}

#[test]
fn test_full_rpy2() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();

    rm_clones("examples/full");
    let (_code, stdout, _stderr) = run_test(
        "examples/full",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import rpy2.robjects as ro; print(ro.r(\"5+5\"));'",
        ],
    );
    assert!(stdout.contains("10"));
}
#[test]
fn test_full_rpy2_sitepaths() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();

    rm_clones("examples/full");
    let (_code, stdout, _stderr) = run_test(
        "examples/full",
        &[
            "test_rpy2"
        ],
    );
    assert!(stdout.contains("Rcpp_1.0.7"));
    assert!(!stdout.contains("Rcpp_1.0.5"));
    assert!(stdout.contains("ACA_1.1"));
}


#[test]
fn test_just_r() {

    let (_code, stdout, _stderr) = run_test(
        "examples/just_r",
        &[
            "run",
            "--",
            "R",
            "-e",
            "'library(Rcpp); sessionInfo()'"
        ],
    );
    assert!(stdout.contains("Rcpp_1.0.7"));
}


#[test]
fn test_flake_with_dir() {

    let (_code, stdout, _stderr) = run_test(
        "examples/flake_in_non_root_github",
        &[
            "run",
            "--",
            "fastq-dump",
            "--version",
        ],
    );
    assert!(stdout.contains("\"fastq-dump\" version 2.11.2"));
}


