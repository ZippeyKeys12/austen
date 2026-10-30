use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::tools::run;

pub(super) fn sha256_file(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    fs::read(path)
        .ok()
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn fetch_verified(path: &Path, url: &str, expected: &str, remedy: &str) {
    if sha256_file(path).is_some_and(|hash| hash == expected) {
        return;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    run(
        Command::new("curl")
            .args(["--fail", "--location", "--retry", "3", "--output"])
            .arg(&partial)
            .arg(url),
        &format!("download {url}"),
        remedy,
    );
    assert!(
        sha256_file(&partial).is_some_and(|hash| hash == expected),
        "SHA-256 mismatch for {url}; expected {expected}"
    );
    fs::rename(&partial, path)
        .unwrap_or_else(|e| panic!("retain the verified download {}: {e}", path.display()));
}
