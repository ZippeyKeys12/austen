use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create bundled runtime directory");
    for entry in fs::read_dir(source).expect("read bundled runtime sources") {
        let entry = entry.expect("read bundled runtime source entry");
        let name = entry.file_name();
        let target = destination.join(if name == "Cargo.toml.in" {
            OsStr::new("Cargo.toml")
        } else {
            name.as_os_str()
        });
        if entry
            .file_type()
            .expect("inspect bundled runtime source")
            .is_dir()
        {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy bundled runtime source");
            let modified = entry.metadata().and_then(|metadata| metadata.modified());
            fs::File::options()
                .write(true)
                .open(&target)
                .and_then(|file| file.set_modified(modified?))
                .expect("preserve bundled runtime source time");
        }
    }
}

pub(super) fn prepare(manifest: &Path) -> Option<PathBuf> {
    let source = manifest.join("runtime_sources");
    if !source.is_dir() {
        return None;
    }
    println!("cargo:rerun-if-changed={}", source.display());
    let destination =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("runtime-sources");
    copy_directory(&source, &destination);
    Some(destination)
}
