# `austen-honggfuzz`: honggfuzz integration

honggfuzz support for AustenTest (`austen test`).
Run coverage-guided mutation against C code while AustenTest checks its specifications and validates saved findings.
The adapter supports C sources and prebuilt targets, automatic persistent execution, a process-per-input mode, and blind mutation for uninstrumented targets.

To use honggfuzz with Austen, install the `austen` CLI with the `honggfuzz` feature as shown below.
Add this crate with `cargo add austen-honggfuzz` only when building your own Rust tooling on the adapter.

## Getting started

Install the CLI with this adapter enabled:

```sh
cargo install austen --locked --features honggfuzz
```

With the native tools below installed, run a campaign against your JSON specification (`spec.json`) and C target (`target.c`):

```sh
austen test spec.json target.c --engine=honggfuzz --duration=30
```

Add `--honggfuzz-mode=stdin` to start a process for each input, or `--honggfuzz-mode=persistent` to require persistent execution.

## Native tools

By default, the adapter uses installed tools without downloading or building honggfuzz.
Put `honggfuzz` on `PATH`, or set `AUSTEN_HONGGFUZZ` to its executable path.
Its matching runtime normally comes from the sibling `hfuzz-clang` tool.
Install Clang separately to compile C targets.
`AUSTEN_HONGGFUZZ_CLANG` selects a particular compiler.

The optional `vendored` feature builds honggfuzz and its matching runtime from a pinned upstream revision.
Enable it through the CLI's `honggfuzz-vendored` feature instead of `honggfuzz`.
It requires Git, Make, a C toolchain, and platform libraries.
On Ubuntu these include `binutils-dev`, `libunwind-dev`, and `libblocksruntime-dev`.
The target must match the host's architecture and OS.
Set `AUSTEN_HONGGFUZZ_SRC` to a local checkout to skip downloading sources.
The tools install under `~/.local/lib/austen/tools/honggfuzz/bin`.
Set `AUSTEN_PREFIX` at build time to change the prefix.
Austen remembers this installation for discovery.
Clang remains a separate dependency.

See the [project overview](https://github.com/ZippeyKeys12/austen/blob/master/README.md), [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it), and [build guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source).
