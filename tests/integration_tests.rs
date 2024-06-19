use named_lock::NamedLock;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn assert_have_github_api_token() {
    if std::env::var("ANYSNAKE2_GITHUB_API_PASSWORD").is_err() {
        panic!("Need to set ANYSNAKE2_GITHUB_API_PASSWORD to run tests");
    }
    if std::env::var("ANYSNAKE2_GITHUB_API_USERNAME").is_err() {
        panic!("Need to set ANYSNAKE2_GITHUB_API_USERNAME to run tests");
    }
}

fn run_test(cwd: &str, args: &[&str]) -> (i32, String, String) {
    assert_have_github_api_token();
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
        .expect("no parent")
        .parent()
        .expect("no parent parent")
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

fn run_test_tempdir(cwd: &str, args: &[&str]) -> ((i32, String, String), TempDir) {
    assert_have_github_api_token();
    let td = tempfile::Builder::new()
        .prefix("anysnake_test")
        .tempdir()
        .expect("could not create tempdir");
    /* std::fs::copy(
        PathBuf::from(&cwd).join("anysnake2.toml"),
        td.path().join("anysnake2.toml"),
    .expect("Could not create anysnake2.toml in tempdir");
    );
    ) */
    let status = Command::new("bash")
        .args([
            "-c",
            &format!("cp {}/* {} -a", cwd, &td.path().to_string_lossy()[..]),
        ])
        .status()
        .expect("bash cp failed");
    if !status.success() {
        panic!("Failed to copy to temp dir");
    }

    (run_test(&td.path().to_string_lossy(), args), td)
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
    // needs to be copied to test the tofu functionality.
    let ((_code, stdout, _stderr), td) = run_test_tempdir(
        "examples/just_python",
        &["run", "--", "python", "--version"],
    );
    assert!(stdout.contains("3.10.4"));

    let td_path = td.path().to_string_lossy();

    let (_code, stdout, stderr) = run_test(
        &td_path,
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import pandas; print(pandas.__version__); import dppd; print(dppd.__version__)'",
        ],
    );

    assert!(stdout.contains("1.5.1"));
    assert!(stdout.contains("0.24"));

    let (_code, stdout, _stderr) = run_test(&td_path, &["run", "--", "hello"]);
    dbg!(&stdout);
    assert!(stdout.contains("Argument strings:"));
    assert!(stdout.contains("loguru version "));
    assert!(!stderr.contains("ImportError"));
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
fn test_basic_pre_post_run() {
    let ((_code, stdout, _stderr), td) = run_test_tempdir("examples/basic", &["test_pre_post"]);
    assert!(stdout.contains("pre_run"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("post_run"));
    //assert order
    assert!(stdout.find("pre_run").unwrap() < stdout.find("run").unwrap());
    assert!(stdout.find("run").unwrap() < stdout.find("post_run").unwrap());

    let while_run_file = td.path().join("while_run.txt");
    assert!(while_run_file.exists());
    let raw = std::fs::read_to_string(while_run_file).unwrap();
    // now make sure that that pid's not in existance
    let pid = raw.trim().parse::<u32>().unwrap();
    assert!(!PathBuf::from(format!("/proc/{}", pid)).exists());
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
    if pb.exists() {
        std::fs::remove_dir_all(pb).unwrap()
    }
}

#[test]
fn test_full() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();

    rm_clones("examples/full");
    let (_code, stdout, _stderr) = run_test("examples/full", &["run", "--", "R", "--version"]);
    assert!(stdout.contains("4.1.1"));
    let out = Command::new("git")
        .args(["log"])
        .current_dir("examples/full/code/dppd_plotnine")
        .output()
        .expect("git log failed");
    assert!(std::str::from_utf8(&out.stdout)
        .unwrap()
        .split('\n')
        .next()
        .unwrap()
        .contains("8ed7651af759f3f0b715a2fbda7bf5119f7145d7"));

    let test_dir = PathBuf::from("examples/full");

    let should_be_there= test_dir.join(".anysnake2_flake/result/rootfs/usr/lib/python3.8/site-packages/pandas/mkpython_args_worked");
    assert!(should_be_there.exists());

    let should_be_there = test_dir.join(".anysnake2_flake/result/rootfs/bin/hello");
    assert!(should_be_there.exists());

    let should_be_there = test_dir.join(".anysnake2_flake/result/rootfs/bin/STAR");
    assert!(should_be_there.exists());

    let rpy2_embededed = test_dir.join(".anysnake2_flake/result/rootfs/usr/lib/python3.8/site-packages/rpy2/rinterface_lib/embedded.py");
    let rpy2_embedded_text = std::fs::read_to_string(rpy2_embededed).unwrap();
    assert!(rpy2_embedded_text.contains("# path to libraries"));
}

#[test]
fn test_full_r_packages() {
    let lock = NamedLock::create("anysnaketest_full").unwrap();
    let _guad = lock.lock().unwrap();
    let test_dir = "examples/full";

    rm_clones(test_dir);
    let (_code, stdout, _stderr) = run_test(
        test_dir,
        &["run", "--", "R", "-e", "'library(ACA);sessionInfo();'"],
    );
    assert!(stdout.contains("ACA_1.1"));

    let override_test_file = PathBuf::from("examples/full")
        .join(".anysnake2_flake/result/rootfs/R_libs/ACA/override_in_place");
    assert!(override_test_file.exists());
    assert_eq!(
        std::fs::read_to_string(override_test_file).unwrap(),
        "Yes\n"
    );
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
    let (_code, stdout, _stderr) = run_test("examples/full", &["test_rpy2"]);
    assert!(stdout.contains("Rcpp_1.0.7"));
    assert!(!stdout.contains("Rcpp_1.0.5"));
    assert!(stdout.contains("ACA_1.1"));
}

#[test]
fn test_just_r() {
    use toml_edit::DocumentMut;
    let toml_path = "examples/just_r/anysnake2.toml";
    let toml = ex::fs::read_to_string(toml_path).unwrap();

    assert!(!toml.contains("nixr_tag"));

    let ((_code, stdout, _stderr), td) = run_test_tempdir(
        "examples/just_r",
        &[
            "run",
            "--",
            "R",
            "-e",
            "'library(Rcpp); library('R6'); library('kedd'); sessionInfo()'",
        ],
    );
    assert!(stdout.contains("Rcpp_1.0.8.3"));
    let override_test_file = PathBuf::from("examples/just_r")
        .join(".anysnake2_flake/result/rootfs/R_libs/Rcpp/override_in_place");
    assert!(override_test_file.exists());
    let toml_path = td.path().join("anysnake2.toml");
    let toml = ex::fs::read_to_string(toml_path).unwrap();
    assert!(toml.contains("nixr_tag ="));
    //verify it's toml
    toml.parse::<DocumentMut>().expect("invalid doc");
}

#[test]
fn test_flake_with_dir() {
    let (_code, stdout, _stderr) = run_test(
        "examples/flake_in_non_root_github",
        &["run", "--", "fastq-dump", "--version"],
    );
    assert!(stdout.contains("\"fastq-dump\" version 2.11.2"));
}

#[test]
fn test_python_package_already_pulled_by_other_editable_package() {
    let (_code, stdout, _stderr) = run_test(
        "examples/test_python_pulled_by_other_editable",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import pypipegraph; print(\"imported ppg\")'",
        ],
    );
    assert!(stdout.contains("imported ppg"));
}

#[test]
fn test_python_pip_reinstall_if_venv_changes() {
    // needs to be copied to test the tofu functionality.
    let ((_code, stdout, _stderr), td) =
        run_test_tempdir("examples/just_python", &["run", "--", "cat"]);
    println!("first: {}", stdout);
    let first =
        ex::fs::read_to_string(td.path().join(".anysnake2_flake/venv/3.10/bin/hello")).unwrap();

    let toml_path = td.path().join("anysnake2.toml");
    let mut toml = ex::fs::read_to_string(&toml_path).unwrap();
    println!("{}", toml);
    toml = toml.replace("pandas=", "solidpython=\"\"\npandas=");
    ex::fs::write(toml_path, toml).unwrap();

    let td_path = td.path().to_string_lossy();
    let (_code, stdout, _stderr) = run_test(&td_path, &["run", "--", "which", "hello"]);
    println!("second: {}", stdout);
    let second =
        ex::fs::read_to_string(td.path().join(".anysnake2_flake/venv/3.10/bin/hello")).unwrap();

    let lines_first: Vec<_> = first.split('\n').collect();
    let lines_second: Vec<_> = second.split('\n').collect();
    assert!(lines_first[0] != lines_second[0]);
    assert!(lines_first[1..] == lines_second[1..]);
}

#[test]
fn test_fetch_from_github_to_fetchgit_transition() {
    {
        let toml_path = "examples/github_tarballs_can_be_unstable/anysnake2.toml";
        let toml = ex::fs::read_to_string(toml_path).unwrap();
        assert!(!toml.contains("hash_6c82cdc20d6f81c96772da73fc07a672a0a0a6ef"));
    }

    let ((_code, stdout, _stderr), td) = run_test_tempdir(
        "examples/github_tarballs_can_be_unstable",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import plotnine; print(plotnine.__version__)'",
        ],
    );
    assert!(stdout.contains("6c82cdc"));
    let toml_path = td.path().join("anysnake2.toml");
    let toml = ex::fs::read_to_string(toml_path).unwrap();
    assert!(toml.contains("plotnine = { method = \"fetchgit\", url = \"https://github.com/has2k1/plotnine\", rev = \"6c82cdc20d6f81c96772da73fc07a672a0a0a6ef\", hash_6c82cdc20d6f81c96772da73fc07a672a0a0a6ef = \"sha256-ORA+GtORqBDhQiwtXUzooqQXostPrQhwHnlD5sW0kTE=\" }"));
}

#[test]
fn test_fetch_trust_on_first_use() {
    {
        let toml_path = "examples/just_python_trust_on_first_use/anysnake2.toml";
        let toml = ex::fs::read_to_string(toml_path).unwrap();

        assert!(!toml.contains("hash_6c82cdc20d6f81c96772da73fc07a672a0a0a6ef = \"sha256-ORA+GtORqBDhQiwtXUzooqQXostPrQhwHnlD5sW0kTE=\" }"));
        assert!(!toml.contains("hash_f42bc1481ed2275427342309d6e876e2d01c3a1a = \"sha256-+TTjvSyR719HQFHU8PNMjWmevDAE5gDKPOeTgcNQ3Bo=\" }"));
    }
    {
        let ((_code, stdout, _stderr), td) = run_test_tempdir(
            "examples/just_python_trust_on_first_use",
            &[
                "run",
                "--",
                "python",
                "-c",
                "'import plotnine; print(plotnine.__version__)'",
            ],
        );
        let toml_path = td.path().join("anysnake2.toml");
        let toml = ex::fs::read_to_string(toml_path).unwrap();
        dbg!(&toml);

        assert!(toml.contains("hash_6c82cdc20d6f81c96772da73fc07a672a0a0a6ef = \"sha256-ORA+GtORqBDhQiwtXUzooqQXostPrQhwHnlD5sW0kTE=\""));
        assert!(toml.contains("hash_db6f0a3254fbd3939d6b6b8c6d1711e7129faba1 = \"sha256-r2yDQ4JuOAZ7oWfjat2R/5OcMi0q7BY1QCK/Z9hyeyY=\""));
        assert!(stdout.contains("6c82cdc"));
    }
}

#[test]
fn test_python_package_from_flake() {
    // needs to be copied to test the tofu functionality.
    let (code, stdout, _stderr) = run_test(
        "examples/just_python_package_from_flake",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import mbf_bam; print(mbf_bam.__version__); print(dir(mbf_bam.mbf_bam))'",
        ],
    );
    assert!(code == 0);
    assert!(stdout.contains("0.2.0"));
    assert!(stdout.contains("count_reads_unstranded"));
}

#[test]
fn test_python_310_nixpkgs_2205() {
    // needs to be copied to test the tofu functionality.
    let ((code, stdout, _stderr), _td) = run_test_tempdir(
        "examples/python_310_nixpkgs_2205/",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import rpy2; print(rpy2.__version__)'",
        ],
    );
    assert!(code == 0);
    assert!(stdout.contains("3.5.5"));
}

#[test]
fn test_python_buildpackage_interdependency_with_overrides() {
    let (code, stdout, _stderr) = run_test(
        "examples/python_buildPackage_interdependency_with_overrides//",
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import testrepo; print(testrepo.__version__); print(testrepo.testrepo2.__version__)'",
        ],
    );
    assert!(code == 0);
    assert!(stdout.contains("0.66"));
    assert!(stdout.contains("0.33"));
}

#[test]
fn test_just_python_pypi() {
    // needs to be copied to test the tofu functionality.
    let ((_code, stdout, _stderr), td) = run_test_tempdir(
        "examples//just_python_package_from_pypi",
        &["run", "--", "python", "--version"],
    );
    assert!(stdout.contains("3.10.4"));

    let td_path = td.path().to_string_lossy();

    let (_code, stdout, _stderr) = run_test(
        &td_path,
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import scanpy; print(scanpy.__version__)'",
        ],
    );

    assert!(stdout.contains("1.9.3"));

    let (_code, stdout, _stderr) = run_test(
        &td_path,
        &[
            "run",
            "--",
            "python",
            "-c",
            "'import dppd; print(\"dppd_version=\", dppd.__version__)'",
        ],
    );
    assert!(stdout.contains("dppd_version="));
    dbg!(&stdout);
    let dppd_version = stdout.trim().split_once("dppd_version=").unwrap().1.trim();
    dbg!(dppd_version);
    let dppd_version: Vec<u32> = dppd_version
        .split('.')
        .map(|x| x.parse::<u32>().unwrap())
        .collect();
    dbg!(&dppd_version);
    assert!(dppd_version >= vec![0u32, 25]);
}
