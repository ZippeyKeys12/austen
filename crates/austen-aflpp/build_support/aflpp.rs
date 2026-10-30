use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use super::{install, native_make, run};

const AFL_VERSION: &str = "v5.03c";

const REPO: &str = "https://github.com/AFLplusplus/AFLplusplus.git";

const TOOLS: [&str; 4] = ["afl-fuzz", "afl-showmap", "afl-tmin", "afl-cmin"];

const REMEDY: &str = "Install git and a C toolchain (make + cc), set AUSTEN_AFLPP_SRC to a \
     local AFL++ checkout, or disable austen's `aflpp-vendored` feature (the tool crate's `vendored` feature) and provide afl-fuzz via \
     AUSTEN_AFL_FUZZ / PATH.";

pub(super) fn build(out_dir: &Path) {
    println!("cargo:rerun-if-env-changed=AUSTEN_AFLPP_SRC");

    let source = match env::var_os("AUSTEN_AFLPP_SRC") {
        Some(dir) => PathBuf::from(dir),
        None => clone_pinned(out_dir),
    };

    println!(
        "cargo:warning=austen: building vendored AFL++ ({AFL_VERSION}) — this is a one-time source build"
    );
    run(
        native_make(&source).args(TOOLS),
        &format!("build AFL++ ({})", TOOLS.join(" ")),
        REMEDY,
    );

    let files = TOOLS.map(|tool| (source.join(tool), tool));
    let bin = install("aflpp", "bin", &files);

    println!(
        "cargo:rustc-env=AUSTEN_AFL_FUZZ_VENDORED={}",
        bin.join("afl-fuzz").display()
    );
}

fn clone_pinned(out_dir: &Path) -> PathBuf {
    let dir = out_dir.join("AFLplusplus");
    if !dir.join(".git").is_dir() {
        run(
            Command::new("git")
                .args(["clone", "--depth", "1", "--branch", AFL_VERSION, REPO])
                .arg(&dir),
            &format!("clone AFL++ {AFL_VERSION}"),
            REMEDY,
        );
    }
    dir
}
