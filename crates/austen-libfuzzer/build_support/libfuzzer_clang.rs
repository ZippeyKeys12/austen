use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const REMEDY: &str = "Install Clang 18 and set AUSTEN_LIBFUZZER_CLANG to its C compiler path \
    (for example /usr/bin/clang-18), or disable austen's `libfuzzer-vendored` feature (the tool crate's `vendored` feature).";

pub(crate) fn discover(
    cxx: &Path,
    override_path: Option<&OsStr>,
    search_path: &OsStr,
    probe: &Path,
) -> Result<PathBuf, String> {
    if let Some(path) = override_path {
        let path = std::path::absolute(Path::new(path))
            .map_err(|e| format!("resolve AUSTEN_LIBFUZZER_CLANG: {e}\n{REMEDY}"))?;
        check(&path, probe).map_err(|e| format!("AUSTEN_LIBFUZZER_CLANG: {e}\n{REMEDY}"))?;
        return Ok(path);
    }

    let cxx_spelled = resolve(cxx, search_path);
    let cxx_real = cxx_spelled.as_ref().and_then(|cxx| cxx.canonicalize().ok());
    let mut candidates: Vec<PathBuf> = Vec::new();
    for candidate in [cxx_spelled, cxx_real]
        .into_iter()
        .flatten()
        .filter_map(|cxx| c_sibling(&cxx))
        .chain(
            ["clang-18", "clang"]
                .into_iter()
                .filter_map(|name| resolve(Path::new(name), search_path)),
        )
    {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    for candidate in candidates {
        if check(&candidate, probe).is_ok() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "no Clang 18 C driver beside {} or on PATH\n{REMEDY}",
        cxx.display()
    ))
}

fn c_sibling(cxx: &Path) -> Option<PathBuf> {
    let name = cxx.file_name()?.to_str()?;
    Some(cxx.with_file_name(if name.contains("clang") {
        name.replacen("clang++", "clang", 1)
    } else {
        "clang".to_owned()
    }))
}

pub(crate) fn is_clang_18(macros: &str) -> bool {
    macros
        .lines()
        .any(|line| line == "#define __clang_major__ 18")
}

fn resolve(command: &Path, search_path: &OsStr) -> Option<PathBuf> {
    let path = if command.components().count() > 1 {
        command.to_owned()
    } else {
        env::split_paths(search_path)
            .map(|directory| directory.join(command))
            .find(|path| executable(path))?
    };
    std::path::absolute(path).ok()
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        metadata.is_file()
    })
}

fn check(path: &Path, probe: &Path) -> Result<(), String> {
    let macros = Command::new(path)
        .args(["-dM", "-E"])
        .arg(probe)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot execute {}: {e}", path.display()))?;
    let output = String::from_utf8_lossy(&macros.stdout);
    if macros.status.success()
        && is_clang_18(&output)
        && !output
            .lines()
            .any(|line| line.starts_with("#define __cplusplus "))
    {
        Ok(())
    } else {
        Err(format!("{} is not a Clang 18 C driver", path.display()))
    }
}
