#[path = "release_support/environment.rs"]
mod environment;

#[cfg(feature = "vendored")]
#[path = "release_support/native_make.rs"]
mod native_make;
#[cfg(feature = "vendored")]
#[path = "release_support/tools.rs"]
mod tools;

#[cfg(feature = "vendored")]
use native_make::native_make;
#[cfg(feature = "vendored")]
use tools::{install, run};

#[cfg(feature = "vendored")]
#[path = "build_support/aflpp.rs"]
mod aflpp;

fn main() {
    environment::declare_rerun();

    #[cfg(feature = "vendored")]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        aflpp::build(&out_dir);
    }
}
