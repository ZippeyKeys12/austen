# `austen-aflpp`: AFL++ integration

AFL++ support for AustenTest (`austen test`).
Use coverage-guided mutation to explore C code while AustenTest checks its specifications and validates saved findings.
The adapter also supports blind mutation, individual finding reduction with `afl-tmin`, and coverage-preserving suite reduction with `afl-cmin`.

To use AFL++ with Austen, install the `austen` CLI with the `aflpp` feature as shown below.
Add this crate with `cargo add austen-aflpp` only when building your own Rust tooling on the adapter.

## Getting started

Install the CLI with this adapter enabled:

```sh
cargo install austen --locked --features aflpp
```

With the native tools below installed, run a campaign against your JSON specification (`spec.json`) and C target (`target.c`):

```sh
austen test spec.json target.c --engine=afl++ --duration=30
```

Add `--shrink=afl-tmin` to reduce findings, or `--minimize-suite` to reduce the resulting suite.
Suite reduction also needs `afl-showmap`.
C sources and prebuilt targets are supported.
Targets without compatible instrumentation run in blind mode.

## Native tools

By default, the adapter uses installed tools without downloading or building AFL++.
Put `afl-fuzz` on `PATH`, or set `AUSTEN_AFL_FUZZ` to its executable path.
Instrumented C-source campaigns need `afl-cc`, found beside the selected fuzzer or on `PATH`.

The optional `vendored` feature builds AFL++ `v5.03c`.
Enable it through the CLI's `aflpp-vendored` feature instead of `aflpp`.
It requires Git, Make, and a C toolchain.
Set `AUSTEN_AFLPP_SRC` to a local checkout to skip downloading sources.
The build installs `afl-fuzz`, `afl-showmap`, `afl-tmin`, and `afl-cmin` under `~/.local/lib/austen/tools/aflpp/bin`.
Set `AUSTEN_PREFIX` at build time to change the prefix.
Austen remembers this installation for discovery.
Vendoring does not build `afl-cc`.
Install it separately for instrumented C targets.

See the [project overview](https://github.com/ZippeyKeys12/austen/blob/master/README.md), [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it), and [build guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source).
