# `austen-libfuzzer`: libFuzzer integration

libFuzzer support for AustenTest (`austen test`).
Use coverage-guided mutation to explore C code while AustenTest checks its specifications and validates saved findings.
libFuzzer runs inside the generated test harness and also provides finding reduction.

To use libFuzzer with Austen, install the `austen` CLI with the `libfuzzer` feature as shown below.
Add this crate with `cargo add austen-libfuzzer` only when building your own Rust tooling on the adapter.

## Getting started

Install the CLI with this adapter enabled:

```sh
cargo install austen --locked --features libfuzzer
```

With the native tools below installed, run a campaign against your JSON specification (`spec.json`) and C target (`target.c`):

```sh
austen test spec.json target.c --engine=libfuzzer --duration=30
```

Direct mode stops at the first crash and retains target state between inputs.
Add `--libfuzzer-mode=fork` to continue after crashes, or `--shrink=libfuzzer` to reduce findings.
Targets need compatible coverage instrumentation.

## Native tools

By default, the adapter uses installed Clang and its matching compiler-rt package without downloading native tools.
It finds `clang` on `PATH` and asks it for the libFuzzer runtime.
Set `AUSTEN_LIBFUZZER_CLANG` to select a C compiler, or `AUSTEN_LIBFUZZER_RUNTIME` to supply a compatible `fuzzer_no_main` archive.

The optional `vendored` feature builds the LLVM 18.1.8 libFuzzer runtime on native Linux.
Enable it through the CLI's `libfuzzer-vendored` feature instead of `libfuzzer`.
It requires Clang 18, its C++ compiler and libstdc++ development headers, plus `ar`, `curl`, `tar`, and `xz`.
The feature builds the runtime and does not install Clang.
Set `CXX=clang++-18` and `AUSTEN_LIBFUZZER_CLANG` to select its compilers if needed.
Set `AUSTEN_LIBFUZZER_SRC` to an LLVM 18 compiler-rt root containing `lib/fuzzer` and `LICENSE.TXT` to skip downloading sources.
The runtime installs under `~/.local/lib/austen/tools/libfuzzer/`.
Set `AUSTEN_PREFIX` at build time to change the prefix.
The adapter remembers the matching C compiler for later use.

See the [project overview](https://github.com/ZippeyKeys12/austen/blob/master/README.md), [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it), and [build guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source).
