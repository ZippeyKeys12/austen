fn main() {
    println!("cargo:rerun-if-changed=src/oracle/opaque_copy.c");

    cc::Build::new()
        .file("src/oracle/opaque_copy.c")
        .warnings_into_errors(true)
        .compile("austen_oracle_opaque_copy");
}
