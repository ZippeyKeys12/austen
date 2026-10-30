use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(super) fn local_inputs(source: &Path, os: &str) -> io::Result<Vec<PathBuf>> {
    let platform = match os {
        "linux" => "linux",
        "macos" => "mac",
        "netbsd" => "netbsd",
        _ => "posix",
    };
    let mut inputs = vec![source.join("Makefile")];
    let directories = [
        "",
        "hfuzz_cc",
        "libhfcommon",
        "libhfuzz",
        "libhfnetdriver",
        platform,
    ];
    for directory in directories
        .into_iter()
        .chain((os == "linux").then_some("posix"))
    {
        for entry in fs::read_dir(source.join(directory))? {
            let path = entry?.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "c" || ext == "h") {
                inputs.push(path);
            }
        }
    }
    if os == "macos" {
        for entry in fs::read_dir(source.join("third_party/mac"))? {
            let path = entry?.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "o") {
                inputs.push(path);
            }
        }
    }
    inputs.sort();
    Ok(inputs)
}
