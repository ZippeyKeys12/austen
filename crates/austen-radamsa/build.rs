#[path = "release_support/environment.rs"]
mod environment;

#[cfg(feature = "vendored")]
#[path = "release_support/download.rs"]
mod download;
#[cfg(feature = "vendored")]
#[path = "release_support/tools.rs"]
mod tools;

#[cfg(feature = "vendored")]
use download::{fetch_verified, sha256_file};
#[cfg(feature = "vendored")]
use tools::{install, run};

#[cfg(feature = "vendored")]
#[path = "build_support/radamsa.rs"]
mod radamsa;

fn main() {
    environment::declare_rerun();

    #[cfg(feature = "vendored")]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        radamsa::build(&out_dir);
    }
}
