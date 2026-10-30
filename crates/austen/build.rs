use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[allow(dead_code)]
#[path = "src/stamp.rs"]
mod stamp;

use stamp::{Stamp, key_for};

#[path = "release_support/install.rs"]
mod install;

#[path = "build_support/runtime_sources.rs"]
mod runtime_sources;

fn library_name() -> &'static str {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        "libausten_harness.dylib"
    } else {
        "libausten_harness.so"
    }
}

fn runtime_root() -> PathBuf {
    let prefix =
        env::var_os("AUSTEN_PREFIX")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| {
            panic!("neither AUSTEN_PREFIX nor HOME is set: nowhere to install the harness runtime")
        });
                home.join(".local")
            });
    prefix.join("lib").join("austen").join("runtime")
}

fn canonical(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

fn engine_cfg_aliases() {
    const TREES: [&str; 3] = ["BENNET", "DARCY", "WICKHAM"];
    const DELEGATED: [&str; 3] = ["AFLPP", "HONGGFUZZ", "LIBFUZZER"];
    const ALL: [&str; 7] = [
        "BENNET",
        "DARCY",
        "WICKHAM",
        "RADAMSA",
        "AFLPP",
        "HONGGFUZZ",
        "LIBFUZZER",
    ];
    let enabled = |feature: &str| env::var_os(format!("CARGO_FEATURE_{feature}")).is_some();
    for (alias, on) in [
        ("austen_tree_engine", TREES.iter().any(|f| enabled(f))),
        (
            "austen_tree_chain",
            TREES.iter().filter(|f| enabled(f)).count() >= 2,
        ),
        (
            "austen_delegated_engines",
            DELEGATED.iter().all(|f| enabled(f)),
        ),
        ("austen_all_engines", ALL.iter().all(|f| enabled(f))),
    ] {
        println!("cargo::rustc-check-cfg=cfg({alias})");
        if on {
            println!("cargo::rustc-cfg={alias}");
        }
    }
}

struct RuntimePackage {
    name: String,
    manifest: PathBuf,
    src: PathBuf,
    build_script: Option<PathBuf>,
}

fn locate_runtime(
    cargo: &str,
    manifest: &Path,
) -> Result<(PathBuf, PathBuf, Vec<RuntimePackage>), String> {
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version=1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .unwrap_or_else(|e| {
            panic!("locate harness runtime workspace with `{cargo} metadata`: {e}")
        });
    if !output.status.success() {
        return Err(format!(
            "`{cargo} metadata` failed for {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("parse cargo metadata for harness runtime workspace");
    let manifest_path = |package: &serde_json::Value| {
        PathBuf::from(package["manifest_path"].as_str().expect("manifest_path"))
    };
    let by_directory: HashMap<PathBuf, &serde_json::Value> = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .map(|package| {
            let mut dir = manifest_path(package);
            dir.pop();
            (dir, package)
        })
        .collect();
    let harness = by_directory
        .iter()
        .find(|(_, package)| package["name"] == "austen-harness")
        .ok_or_else(|| {
            format!(
                "the workspace of {} has no package `austen-harness`",
                manifest.display()
            )
        })?;
    let harness_manifest = manifest_path(harness.1);
    let mut pending = vec![harness.0.clone()];
    let mut visited = HashSet::new();
    let mut packages = Vec::new();
    while let Some(dir) = pending.pop() {
        if !visited.insert(dir.clone()) {
            continue;
        }
        let package = by_directory.get(&dir).unwrap_or_else(|| {
            panic!(
                "runtime path dependency is not a workspace member: {}",
                dir.display()
            )
        });
        let targets = package["targets"].as_array().expect("package targets");
        let target_path = |kinds: &[&str]| {
            targets
                .iter()
                .find(|target| {
                    target["kind"].as_array().is_some_and(|declared| {
                        declared
                            .iter()
                            .any(|kind| kind.as_str().is_some_and(|kind| kinds.contains(&kind)))
                    })
                })
                .map(|target| PathBuf::from(target["src_path"].as_str().expect("src_path")))
        };
        let mut src = target_path(&["lib", "rlib", "dylib", "cdylib", "staticlib"])
            .unwrap_or_else(|| panic!("runtime package {} has no library", dir.display()));
        src.pop();
        pending.extend(
            package["dependencies"]
                .as_array()
                .expect("package dependencies")
                .iter()
                .filter(|dependency| dependency["kind"] != "dev")
                .filter_map(|dependency| dependency["path"].as_str().map(PathBuf::from)),
        );
        packages.push(RuntimePackage {
            name: package["name"].as_str().expect("package name").to_owned(),
            manifest: manifest_path(package),
            src,
            build_script: target_path(&["custom-build"]),
        });
    }
    let root = PathBuf::from(
        metadata["workspace_root"]
            .as_str()
            .expect("metadata workspace_root"),
    );
    Ok((root, harness_manifest, packages))
}

fn main() {
    println!("cargo:rerun-if-env-changed=AUSTEN_PREFIX");
    engine_cfg_aliases();

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut bundled = None;
    let (workspace, harness_manifest, runtime_packages) = match locate_runtime(
        &cargo,
        &manifest.join("Cargo.toml"),
    ) {
        Ok(located) => located,
        Err(reason) => {
            let bundle = runtime_sources::prepare(&manifest).unwrap_or_else(|| {
                    panic!(
                        "locate harness runtime workspace: {reason}; and the package ships no bundled `runtime_sources`"
                    )
                });
            let located = locate_runtime(&cargo, &bundle.join("Cargo.toml"))
                .unwrap_or_else(|reason| panic!("bundled runtime workspace: {reason}"));
            bundled = Some(bundle);
            located
        }
    };
    for package in &runtime_packages {
        println!("cargo:rerun-if-changed={}", package.manifest.display());
        if let Some(build_script) = &package.build_script {
            println!("cargo:rerun-if-changed={}", build_script.display());
        }
        if package.name == "austen-lib" {
            for module in ["gen_dsl", "spec", "wire", "lib.rs"] {
                println!(
                    "cargo:rerun-if-changed={}",
                    package.src.join(module).display()
                );
            }
        } else {
            println!("cargo:rerun-if-changed={}", package.src.display());
        }
    }
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("Cargo.toml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("Cargo.lock").display()
    );
    println!("cargo:rerun-if-changed=src/harness_main.rs.in");

    let host = env::var("HOST").expect("HOST set by cargo");
    println!("cargo:rustc-env=AUSTEN_HOST={host}");
    println!(
        "cargo:rustc-env=AUSTEN_BUILD_PROFILE={}",
        env::var("PROFILE").expect("PROFILE set by cargo")
    );
    let source = canonical(if bundled.is_some() {
        manifest.as_path()
    } else {
        workspace.as_path()
    });
    println!("cargo:rustc-env=AUSTEN_SOURCE={}", source.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let lib_name = library_name();

    let runtime_target = out_dir
        .ancestors()
        .nth(4)
        .map_or_else(
            || out_dir.join("runtime-build"),
            |target| target.join("austen-runtime-build"),
        )
        .join(key_for(&source));
    let status = Command::new(&cargo)
        .arg("build")
        .arg("--locked")
        .arg("--release")
        .arg("--lib")
        .arg("--manifest-path")
        .arg(&harness_manifest)
        .arg("--target-dir")
        .arg(&runtime_target)
        .env("LIBAFL_EDGES_MAP_ALLOCATED_SIZE", "65536")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .status()
        .unwrap_or_else(|e| panic!("run `{cargo} build` for the harness runtime: {e}"));
    assert!(status.success(), "building the harness runtime failed");

    let built = runtime_target.join("release").join(lib_name);
    assert!(
        built.is_file(),
        "the harness runtime build produced no {}",
        built.display()
    );

    let root = runtime_root();
    let runtime = root.join(key_for(&source));
    let stamp = Stamp {
        version: env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION"),
        host: host.clone(),
        profile: "release".to_string(),
        source: source.display().to_string(),
    };
    let stamp_path = out_dir.join("RUNTIME");
    fs::write(&stamp_path, stamp.render())
        .unwrap_or_else(|e| panic!("write runtime stamp {}: {e}", stamp_path.display()));
    let runtime = install::install_into(
        &runtime,
        &[
            (built, lib_name),
            (
                manifest.join("src/harness_main.rs.in"),
                "harness_main.rs.in",
            ),
            (stamp_path, "RUNTIME"),
        ],
    )
    .unwrap_or_else(|e| {
        panic!(
            "install harness runtime into {}: {e}\n\
             Set AUSTEN_PREFIX to a writable prefix and rebuild.",
            runtime.display()
        )
    });

    println!(
        "cargo:rustc-env=AUSTEN_RUNTIME_DEFAULT={}",
        runtime.display()
    );
}
