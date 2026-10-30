pub(super) const NATIVE_TOOLCHAIN: [&str; 4] = ["CC", "CFLAGS", "LDFLAGS", "AR"];

pub(super) fn declare_rerun() {
    println!("cargo:rerun-if-env-changed=AUSTEN_PREFIX");
    if cfg!(feature = "vendored") {
        for name in NATIVE_TOOLCHAIN {
            println!("cargo:rerun-if-env-changed={name}");
        }
    }
}
