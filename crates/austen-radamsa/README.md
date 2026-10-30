# `austen-radamsa`: Radamsa input generation

[Radamsa](https://gitlab.com/akihe/radamsa) support for AustenTest (`austen test`).
Radamsa mutates seed inputs while AustenTest runs the C target, checks its specifications, and saves findings.
It supplies raw input bytes through a persistent process and does not use target coverage to guide mutation.

To use Radamsa with Austen, install the `austen` CLI with the `radamsa` feature as shown below.
Add this crate with `cargo add austen-radamsa` only when building your own Rust tooling on the adapter.

## Getting started

Install the CLI with this adapter enabled:

```sh
cargo install austen --locked --features radamsa
```

With the native tools below installed, run a campaign against your JSON specification (`spec.json`) and C target (`target.c`):

```sh
austen test spec.json target.c --engine=radamsa --duration=30
```

Use `--seed` to select the mutation seed.
Saved inputs can be replayed through the usual `--input` workflow.

## Native tools

This adapter requires **Radamsa 0.7**.
Put `radamsa` on `PATH`, or set `AUSTEN_RADAMSA` to its executable path.
Austen checks that the selected executable reports exactly `Radamsa 0.7`.
By default, this crate does not download or build Radamsa.

The optional `vendored` feature builds the pinned generated-C release.
Enable it through the CLI's `radamsa-vendored` feature instead of `radamsa`.
It requires a native C compiler, `curl`, and `gzip`.
The target architecture and OS must match the host.
Set `CC` and `CFLAGS` to customize compilation.
For an offline build, set `AUSTEN_RADAMSA_SRC` to a directory containing `radamsa.c`, `RADAMSA-LICENCE`, and `OWL-LICENCE` from the generated-C release.
The executable installs under `~/.local/lib/austen/tools/radamsa/bin/`.
Set `AUSTEN_PREFIX` at build time to change the prefix.
Clang remains a separate dependency for compiling instrumented C targets.

See the [project overview](https://github.com/ZippeyKeys12/austen/blob/master/README.md), [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it), and [build guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source).
