use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) fn install_into(directory: &Path, files: &[(PathBuf, &str)]) -> io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let directory = directory.canonicalize()?;
    for (source, name) in files {
        replace(source, &directory.join(name))?;
    }
    Ok(directory)
}

fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    let mut input = File::open(source)?;
    let directory = destination
        .parent()
        .expect("an installed file has a parent");
    let (mut pending, mut output) = loop {
        let serial = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(".austen-install-{}-{serial}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (PendingFile(Some(path)), file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    io::copy(&mut input, &mut output)?;
    output.set_permissions(input.metadata()?.permissions())?;
    drop(output);
    fs::rename(pending.0.as_ref().unwrap(), destination)?;
    pending.0 = None;
    Ok(())
}

struct PendingFile(Option<PathBuf>);

impl Drop for PendingFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
