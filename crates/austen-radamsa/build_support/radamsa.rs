use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{fetch_verified, install, run, sha256_file};

#[path = "../release_support/cache.rs"]
mod cache;

const VERSION: &str = "0.7";
const SOURCE_REV: &str = "40d5dec416fb5277dbbd72c04b82ba2ae039778a";
const OWL_REV: &str = "2f7e001540485c77b5f744081e2f5f121fa7eb3a";
const HEX_REV: &str = "68fd717471ef063910161ee3e03609a910ca1314";
const OWL_CODEC_REV: &str = "a7f7df5ff958d68731dcd715a61878193a15ff63";
const ASSET_URL: &str =
    "https://gitlab.com/akihe/radamsa/uploads/d774a42f7893012d0a56c490a75ae12b/radamsa-0.7.c.gz";
const ASSET_SHA256: &str = "79d90cb6290bfeda51d42c049841788a5e9c0d74c7b7b0766f97394a5b0c0055";
const LICENCES: &[(&str, &str, &str, &str)] = &[
    (
        "RADAMSA-LICENCE",
        "akihe/radamsa",
        SOURCE_REV,
        "4f31f940cb888ab4f6519774ee220f8c1b27871f8d278812b2572185e0013d96",
    ),
    (
        "OWL-LICENCE",
        "owl-lisp/owl",
        OWL_REV,
        "9b8546be23fd7b6c022298e72e55d74a46acee15548196a4f8b2f86da32b80ed",
    ),
];
const REMEDY: &str = "Install a C compiler, curl, and gzip; set AUSTEN_RADAMSA_SRC to a local \
    directory containing radamsa.c, RADAMSA-LICENCE, and OWL-LICENCE to skip downloads; \
    or disable austen's `radamsa-vendored` feature (the tool crate's `vendored` feature) and provide Radamsa 0.7 via AUSTEN_RADAMSA / PATH.";

pub(super) fn build(out_dir: &Path) {
    println!("cargo:rerun-if-env-changed=AUSTEN_RADAMSA_SRC");
    assert!(
        env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok(env::consts::OS)
            && env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok(env::consts::ARCH),
        "vendored Radamsa is built for the host; target architecture and OS must match; disable austen's `radamsa-vendored` feature (the tool crate's `vendored` feature) when cross-compiling"
    );

    let required = || std::iter::once("radamsa.c").chain(LICENCES.iter().map(|(name, ..)| *name));
    let (source, origin) = match env::var_os("AUSTEN_RADAMSA_SRC") {
        Some(path) => {
            let source = PathBuf::from(path);
            for name in required() {
                println!("cargo:rerun-if-changed={}", source.join(name).display());
            }
            let origin = format!("Local generated-C source override: {}", source.display());
            (source, origin)
        }
        None => (
            fetch_source(out_dir),
            format!(
                "Radamsa version: {VERSION}\nRadamsa tag: v{VERSION}\n\
                 Radamsa source commit: {SOURCE_REV}\nGenerated-C asset: {ASSET_URL}\n\
                 Generated-C gzip SHA-256: {ASSET_SHA256}\n\
                 Embedded Owl version/license commit: 0.2.2 / {OWL_REV}\n\
                 hex extraction: https://gitlab.com/owl-lisp/hex/-/commit/{HEX_REV}\n\
                 MIT-covered Owl codec source: https://gitlab.com/owl-lisp/owl/-/blob/{OWL_CODEC_REV}/owl/codec.scm"
            ),
        ),
    };
    for name in required() {
        let path = source.join(name);
        assert!(path.is_file(), "missing {}\n{REMEDY}", path.display());
    }
    let c_source = source.join("radamsa.c");
    let c_hash = sha256_file(&c_source).expect("hash the Radamsa C source");

    let host = env::var("HOST").expect("HOST");
    let compiler = cc::Build::new()
        .host(&host)
        .target(&host)
        .out_dir(out_dir)
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .inherit_rustflags(false)
        .try_get_compiler()
        .unwrap_or_else(|e| panic!("find the Radamsa C compiler: {e}\n{REMEDY}"));
    let build = out_dir.join("radamsa-build");
    fs::create_dir_all(&build).expect("create the Radamsa build directory");
    let executable = build.join("radamsa");
    let mut compile = compiler.to_command();
    compile.arg(&c_source).arg("-o").arg(&executable);
    let stamp = build.join(".austen-built");
    let key = format!("{c_hash}\n{compile:?}");
    cache::update(&stamp, &key, &executable, || {
        println!("cargo:warning=austen: building vendored Radamsa ({VERSION})");
        run(&mut compile, "compile Radamsa", REMEDY);
    });

    let provenance = build.join("PROVENANCE");
    fs::write(
        &provenance,
        format!(
            "{origin}\nGenerated-C SHA-256: {c_hash}\nBuild compiler: {}\nBuild host: {host}\n",
            compiler.path().display(),
        ),
    )
    .expect("write Radamsa provenance");
    let notices: Vec<_> = LICENCES
        .iter()
        .map(|(name, ..)| (source.join(name), *name))
        .chain([(provenance, "PROVENANCE")])
        .collect();
    install("radamsa", "share/licenses", &notices);
    let bin = install("radamsa", "bin", &[(executable, "radamsa")]);
    println!(
        "cargo:rustc-env=AUSTEN_RADAMSA_VENDORED={}",
        bin.join("radamsa").display()
    );
}

fn fetch_source(out_dir: &Path) -> PathBuf {
    let source = out_dir.join(format!("radamsa-{VERSION}-source"));
    fs::create_dir_all(&source).expect("create the Radamsa source cache");
    for (name, project, rev, hash) in LICENCES {
        let url = format!("https://gitlab.com/{project}/-/raw/{rev}/LICENCE");
        fetch_verified(&source.join(name), &url, hash, REMEDY);
    }
    let marker = source.join(".austen-extracted");
    let c_source = source.join("radamsa.c");
    cache::update(&marker, ASSET_SHA256, &c_source, || {
        let archive = source.join("radamsa.c.gz");
        fetch_verified(&archive, ASSET_URL, ASSET_SHA256, REMEDY);
        run(
            Command::new("gzip")
                .arg("-dc")
                .arg(&archive)
                .stdout(fs::File::create(&c_source).expect("create Radamsa C source")),
            "extract the Radamsa generated-C release",
            REMEDY,
        );
    });
    source
}
