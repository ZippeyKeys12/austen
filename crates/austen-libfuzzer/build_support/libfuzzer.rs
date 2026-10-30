use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{fetch_verified, install, run, sha256_file};

#[path = "../release_support/cache.rs"]
mod cache;
#[path = "libfuzzer_clang.rs"]
mod clang;

const VERSION: &str = "18.1.8";
const URL: &str = "https://github.com/llvm/llvm-project/releases/download/llvmorg-18.1.8/compiler-rt-18.1.8.src.tar.xz";
const SHA256: &str = "e054e99a9c9240720616e927cb52363abbc8b4f1ef0286bad3df79ec8fdf892f";
const ARCHIVE: &str = "libclang_rt.fuzzer_no_main.a";
const REMEDY: &str = "Install Clang 18, C++ standard-library headers, ar, curl, tar, and xz; \
    set CXX to clang++-18 or AUSTEN_LIBFUZZER_SRC to a local LLVM 18 compiler-rt \
    source directory, or disable austen's `libfuzzer-vendored` feature (the tool crate's `vendored` feature) and use an installed compiler-rt.";

pub(super) const SOURCES: &[&str] = &[
    "FuzzerCrossOver.cpp",
    "FuzzerDataFlowTrace.cpp",
    "FuzzerDriver.cpp",
    "FuzzerExtFunctionsDlsym.cpp",
    "FuzzerExtFunctionsWeak.cpp",
    "FuzzerExtFunctionsWindows.cpp",
    "FuzzerExtraCounters.cpp",
    "FuzzerExtraCountersDarwin.cpp",
    "FuzzerExtraCountersWindows.cpp",
    "FuzzerFork.cpp",
    "FuzzerIO.cpp",
    "FuzzerIOPosix.cpp",
    "FuzzerIOWindows.cpp",
    "FuzzerLoop.cpp",
    "FuzzerMerge.cpp",
    "FuzzerMutate.cpp",
    "FuzzerSHA1.cpp",
    "FuzzerTracePC.cpp",
    "FuzzerUtil.cpp",
    "FuzzerUtilDarwin.cpp",
    "FuzzerUtilFuchsia.cpp",
    "FuzzerUtilLinux.cpp",
    "FuzzerUtilPosix.cpp",
    "FuzzerUtilWindows.cpp",
];

pub(super) fn build(out_dir: &Path) {
    println!("cargo:rerun-if-env-changed=AUSTEN_LIBFUZZER_SRC");
    println!("cargo:rerun-if-env-changed=AUSTEN_LIBFUZZER_CLANG");
    assert!(
        env::consts::OS == "linux"
            && env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
            && env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok(env::consts::ARCH),
        "vendored libFuzzer supports native Linux builds only; disable austen's `libfuzzer-vendored` feature (the tool crate's `vendored` feature) when cross-compiling or building for another OS"
    );

    let host = env::var("HOST").expect("HOST");
    configure_build_environment(&host);
    let compiler_vars = [
        format!("CXX_{host}"),
        format!("CXX_{}", host.replace(['-', '.'], "_")),
        "HOST_CXX".to_string(),
        "CXX".to_string(),
    ];
    for name in &compiler_vars {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let mut native = cc::Build::new();
    native
        .cpp(true)
        .host(&host)
        .target(&host)
        .out_dir(out_dir)
        .inherit_rustflags(false)
        .warnings(false);
    if compiler_vars.iter().all(|name| env::var_os(name).is_none()) {
        native.compiler(
            if Command::new("clang++-18").arg("--version").output().is_ok() {
                "clang++-18"
            } else {
                "clang++"
            },
        );
    }
    let compiler = native
        .try_get_compiler()
        .unwrap_or_else(|e| panic!("find the libFuzzer C++ compiler: {e}\n{REMEDY}"));
    let macros = compiler
        .to_command()
        .args(["-dM", "-E", "-x", "c++", "/dev/null"])
        .output()
        .unwrap_or_else(|e| panic!("check the libFuzzer C++ compiler: {e}\n{REMEDY}"));
    assert!(
        macros.status.success() && clang::is_clang_18(&String::from_utf8_lossy(&macros.stdout)),
        "libFuzzer {VERSION} requires Clang 18; set CXX to clang++-18\n{REMEDY}"
    );
    let probe = out_dir.join("libfuzzer-driver-check.c");
    fs::write(&probe, "").expect("write the libFuzzer C compiler probe");
    let clang = clang::discover(
        compiler.path(),
        env::var_os("AUSTEN_LIBFUZZER_CLANG").as_deref(),
        &env::var_os("PATH").unwrap_or_default(),
        &probe,
    )
    .unwrap_or_else(|e| panic!("find the libFuzzer C compiler: {e}"));
    println!("cargo:rerun-if-changed={}", clang.display());
    println!(
        "cargo:rustc-env=AUSTEN_LIBFUZZER_CLANG_VENDORED={}",
        clang.display()
    );

    let source = match env::var_os("AUSTEN_LIBFUZZER_SRC") {
        Some(path) => {
            let source = PathBuf::from(path);
            track_local_source(&source);
            source
        }
        None => fetch_source(out_dir),
    };
    assert!(
        source.join("lib/fuzzer/FuzzerDriver.cpp").is_file()
            && source.join("LICENSE.TXT").is_file(),
        "{} is not a compiler-rt source root (expected lib/fuzzer and LICENSE.TXT)\n{REMEDY}",
        source.display()
    );
    let build = out_dir.join("libfuzzer-build");
    fs::create_dir_all(&build).expect("create the libFuzzer build directory");
    native.out_dir(&build).files(
        SOURCES
            .iter()
            .map(|file| source.join("lib/fuzzer").join(file)),
    );
    let archive = build_archive(&native, &source, &build);

    let lib = install("libfuzzer", "lib", &[(archive, ARCHIVE)]);
    install(
        "libfuzzer",
        "share/licenses",
        &[(source.join("LICENSE.TXT"), "LLVM-LICENSE.TXT")],
    );
    println!(
        "cargo:rustc-env=AUSTEN_LIBFUZZER_VENDORED={}",
        lib.join(ARCHIVE).display()
    );
}

fn fetch_source(out_dir: &Path) -> PathBuf {
    let source = out_dir.join(format!("compiler-rt-{VERSION}.src"));
    let marker = source.join(".austen-extracted");
    cache::update(
        &marker,
        SHA256,
        &source.join("lib/fuzzer/FuzzerDriver.cpp"),
        || {
            let archive = out_dir.join(format!("compiler-rt-{VERSION}.src.tar.xz"));
            fetch_verified(&archive, URL, SHA256, REMEDY);
            run(
                Command::new("tar")
                    .arg("-xJf")
                    .arg(&archive)
                    .arg("-C")
                    .arg(out_dir),
                "extract compiler-rt",
                REMEDY,
            );
        },
    );
    source
}

fn source_inputs(source: &Path) -> Vec<PathBuf> {
    let fuzzer = source.join("lib/fuzzer");
    let mut inputs: Vec<_> = fs::read_dir(&fuzzer)
        .unwrap_or_else(|e| panic!("read {}: {e}\n{REMEDY}", fuzzer.display()))
        .map(|entry| entry.expect("read a libFuzzer source entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "h" || extension == "def")
        })
        .chain(SOURCES.iter().map(|file| fuzzer.join(file)))
        .collect();
    inputs.sort();
    inputs
}

fn track_local_source(source: &Path) {
    for path in source_inputs(source)
        .into_iter()
        .chain([source.join("LICENSE.TXT")])
    {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn configure_build_environment(host: &str) {
    let flags_name = format!("CXXFLAGS_{host}");
    let mut flags = env::var_os(&flags_name).unwrap_or_default();
    let shell_escaped = env::var_os("CC_SHELL_ESCAPED_FLAGS").is_some_and(|value| {
        !value.is_empty() && value != "0" && value != "false" && value != "no"
    });
    if shell_escaped {
        let words = shlex::split(&flags.to_string_lossy())
            .unwrap_or_else(|| panic!("invalid shell quoting in {flags_name}\n{REMEDY}"));
        flags = shlex::try_join(words.iter().map(String::as_str))
            .expect("environment flags cannot contain NUL bytes")
            .into();
    }
    flags.push(" -std=c++17 -O2 -fno-omit-frame-pointer -fPIC -fno-sanitize=all -fsanitize-coverage=0 -fno-lto -stdlib=libstdc++");
    unsafe { env::set_var(flags_name, flags) };
    if env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse::<u32>().ok())
        == Some(0)
    {
        unsafe { env::set_var("NUM_JOBS", "1") };
    }
}

fn build_archive(native: &cc::Build, source: &Path, build: &Path) -> PathBuf {
    let compiler = native
        .try_get_compiler()
        .unwrap_or_else(|e| panic!("find the libFuzzer C++ compiler: {e}\n{REMEDY}"));
    let mut archiver = native.get_archiver();
    let archive = build.join(ARCHIVE);
    archiver.arg("crs").arg(&archive);
    let mut key = format!(
        "LLVM {VERSION}\n{:?}\n{archiver:?}\n",
        compiler.to_command()
    );
    for tool in [
        compiler.path(),
        Path::new(compiler.to_command().get_program()),
        Path::new(archiver.get_program()),
    ] {
        let resolved = if tool.components().count() > 1 || tool.is_absolute() {
            tool.to_path_buf()
        } else {
            env::split_paths(&env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(tool))
                .find(|path| path.is_file())
                .unwrap_or_else(|| tool.to_path_buf())
        };
        println!("cargo:rerun-if-changed={}", resolved.display());
        key.push_str(&format!(
            "{resolved:?} {:?}\n",
            sha256_file(&resolved).expect("hash the libFuzzer build tool")
        ));
    }
    for path in source_inputs(source) {
        key.push_str(&format!(
            "{path:?} {}\n",
            sha256_file(&path).unwrap_or_else(|| panic!("hash {}\n{REMEDY}", path.display()))
        ));
    }
    let stamp = build.join(".austen-built");
    cache::update(&stamp, &key, &archive, || {
        println!("cargo:warning=austen: building vendored libFuzzer (LLVM {VERSION})");
        let objects = native
            .try_compile_intermediates()
            .unwrap_or_else(|e| panic!("compile libFuzzer: {e}\n{REMEDY}"));
        if archive.exists() {
            fs::remove_file(&archive).expect("remove the previous libFuzzer archive");
        }
        run(archiver.args(objects), "archive libFuzzer", REMEDY);
    });
    archive
}
