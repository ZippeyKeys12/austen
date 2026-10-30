pub(super) fn native_make(source: &std::path::Path) -> std::process::Command {
    let mut make = std::process::Command::new("make");
    make.current_dir(source)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_MAKEFLAGS");
    if let Ok(jobs) = std::env::var("NUM_JOBS") {
        make.arg(format!("-j{jobs}"));
    }
    make
}
