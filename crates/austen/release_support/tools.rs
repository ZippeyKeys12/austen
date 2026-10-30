use std::{env, path::PathBuf, process::Command};

#[path = "install.rs"]
mod install;

fn tools_dir(tool: &str) -> PathBuf {
    let prefix = env::var_os("AUSTEN_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = env::var_os("HOME")
                .map(PathBuf::from)
                .expect("neither AUSTEN_PREFIX nor HOME is set: nowhere to install vendored tools");
            home.join(".local")
        });
    prefix.join("lib").join("austen").join("tools").join(tool)
}

pub(super) fn run(command: &mut Command, action: &str, remedy: &str) {
    let status = command
        .status()
        .unwrap_or_else(|e| panic!("{action}: {e}\n{remedy}"));
    assert!(status.success(), "{action}: {status}\n{remedy}");
}

pub(super) fn install(tool: &str, dir: &str, files: &[(PathBuf, &str)]) -> PathBuf {
    let bin = tools_dir(tool).join(dir);
    for (built, _) in files {
        assert!(
            built.is_file(),
            "the {tool} build produced no {} — try a clean source checkout",
            built.display()
        );
    }
    install::install_into(&bin, files).unwrap_or_else(|e| {
        panic!(
            "install {tool} into {}: {e}\nSet AUSTEN_PREFIX to a writable prefix and rebuild.",
            bin.display()
        )
    })
}
