use std::{fs, path::Path};

pub(super) fn update(stamp: &Path, key: &str, output: &Path, build: impl FnOnce()) {
    if output.is_file() && fs::read_to_string(stamp).is_ok_and(|built| built == key) {
        return;
    }
    if let Err(error) = fs::remove_file(stamp)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        panic!("invalidate {}: {error}", stamp.display());
    }
    build();
    assert!(
        output.is_file(),
        "operation produced no {}",
        output.display()
    );
    fs::write(stamp, key).unwrap_or_else(|error| panic!("record {}: {error}", stamp.display()));
}
