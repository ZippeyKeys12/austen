use std::{
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

use super::{environment::NATIVE_TOOLCHAIN, install, native_make, run};

#[path = "honggfuzz_sources.rs"]
mod sources;

const HONGGFUZZ_REV: &str = "cf8b66a4d09f4d4d786d96e3c46d9141fb4e98e2";
const HONGGFUZZ_REPO: &str = "https://github.com/google/honggfuzz.git";

const REMEDY: &str = "Install the required build tools/dependencies, set AUSTEN_HONGGFUZZ_SRC to \
     a local honggfuzz checkout to skip the download, or disable austen's `honggfuzz-vendored` feature (the tool crate's `vendored` feature) and \
     provide honggfuzz via AUSTEN_HONGGFUZZ / PATH.";

pub(super) fn build(out_dir: &Path) {
    println!("cargo:rerun-if-env-changed=AUSTEN_HONGGFUZZ_SRC");
    let target = (
        env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH"),
        env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS"),
    );
    assert_eq!(
        (env::consts::ARCH, env::consts::OS),
        (target.0.as_str(), target.1.as_str()),
        "vendored honggfuzz is built for the host, so its executable and runtime archives must match the target's architecture and OS; disable austen's `honggfuzz-vendored` feature (the tool crate's `vendored` feature) when cross-compiling"
    );
    let source = match env::var_os("AUSTEN_HONGGFUZZ_SRC") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            for path in sources::local_inputs(&dir, env::consts::OS).unwrap_or_else(|e| {
                panic!(
                    "inspect honggfuzz sources in {}: {e}\n{REMEDY}",
                    dir.display()
                )
            }) {
                println!("cargo:rerun-if-changed={}", path.display());
            }
            dir
        }
        None => clone_honggfuzz(out_dir),
    };
    println!(
        "cargo:warning=austen: building vendored honggfuzz ({HONGGFUZZ_REV}) — this is a one-time source build"
    );

    let build = build_dir(out_dir, &source);
    run(
        native_make(&source)
            .arg(format!("BUILD_DIR={}", build.display()))
            .arg(build.join("honggfuzz"))
            .arg(build.join("hfuzz_cc/hfuzz-cc")),
        "build honggfuzz and hfuzz-clang",
        REMEDY,
    );

    let bin = install(
        "honggfuzz",
        "bin",
        &[
            (build.join("honggfuzz"), "honggfuzz"),
            (build.join("hfuzz_cc/hfuzz-cc"), "hfuzz-clang"),
        ],
    );
    println!(
        "cargo:rustc-env=AUSTEN_HONGGFUZZ_VENDORED={}",
        bin.join("honggfuzz").display()
    );
}

fn build_dir(out_dir: &Path, source: &Path) -> PathBuf {
    let mut key = DefaultHasher::new();
    source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf())
        .hash(&mut key);
    HONGGFUZZ_REV.hash(&mut key);
    let makefile = source.join("Makefile");
    fs::read(&makefile)
        .unwrap_or_else(|e| panic!("read {}: {e}\n{REMEDY}", makefile.display()))
        .hash(&mut key);
    for name in NATIVE_TOOLCHAIN {
        env::var_os(name).hash(&mut key);
    }
    out_dir.join(format!("honggfuzz-build-{:016x}", key.finish()))
}

fn clone_honggfuzz(out_dir: &Path) -> PathBuf {
    let dir = out_dir.join(format!("honggfuzz-{HONGGFUZZ_REV}"));
    if !dir.join(".git").is_dir() {
        run(
            Command::new("git").arg("init").arg(&dir),
            "initialize the honggfuzz source cache",
            REMEDY,
        );
    }
    let have_pin = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["cat-file", "-e", HONGGFUZZ_REV])
        .output()
        .is_ok_and(|out| out.status.success());
    if !have_pin {
        run(
            Command::new("git").arg("-C").arg(&dir).args([
                "fetch",
                "--depth",
                "1",
                HONGGFUZZ_REPO,
                HONGGFUZZ_REV,
            ]),
            "fetch the pinned honggfuzz commit",
            REMEDY,
        );
    }
    run(
        Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["checkout", "--detach", HONGGFUZZ_REV]),
        "check out the pinned honggfuzz commit",
        REMEDY,
    );
    dir
}
