# `austen`: Command-line interface

The Austen command-line application for working with C code and its JSON specifications.
Its main tool, AustenTest (`austen test`), turns a specification and its C target into a runnable test harness, generates inputs, and saves findings for replay.

Use `austen` to:

- Check a specification for potentially incomplete specifications with `austen sanity`.
- Run a testing campaign with `austen test`.
- Replay, reduce, and report crashes and postcondition failures.

## Installation

Install the command-line application from crates.io:

```sh
cargo install austen --locked --features bennet,darcy
austen --help
```

Building requires Rust 1.93.1 or newer and a C toolchain.
The recommended install above enables Bennet and Darcy.
Darcy requires Z3 5.1.0 with matching native library and `pkg-config` metadata.
Use `--features bennet` for a build without Z3.
Installation also builds and stages the shared harness runtime under `~/.local/lib/austen/runtime/`.
Set `AUSTEN_PREFIX` when installing to choose another runtime prefix.
Keep `rustc` and a C compiler available to build harnesses for new targets.

## Getting started

Given a JSON specification `spec.json` and its C target `target.c`:

```sh
austen sanity spec.json
austen test spec.json target.c --duration=30 --work=austen-run
```

The sanity check reports potentially incomplete specifications without running the target.
The testing campaign saves findings for replay and reports coverage.
See the [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it) for exporting specifications, replaying inputs, and reporting results in CI.

## Choosing engines

All engines are opt-in Cargo features.
With Bennet and Darcy enabled, Austen starts each function with Bennet, switching to Darcy when generation repeatedly discards inputs.
Select one directly with `--engine=bennet` or `--engine=darcy`.

The engine features are `bennet`, `darcy`, `wickham`, `radamsa`, `aflpp`, `honggfuzz`, and `libfuzzer`.
Use `all-engines` to include every adapter.
External tools must be installed separately.
The corresponding `*-vendored` features build supported pinned tools or runtimes.

For example, install Wickham alongside Bennet and Darcy with:

```sh
cargo install austen --locked --features bennet,darcy,wickham
```

Installing without engine features retains help, version, specification sanity checks, and exact-input execution.
If `austen test` has no default engine available, it recommends installing Bennet and Darcy with `cargo install austen --features bennet,darcy`.

The [build guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source) describes native dependencies and tool setup.
The [austen-harness](https://crates.io/crates/austen-harness) crate provides the runtime used by generated harnesses.

## Documentation

- [User guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it): installation, engine choices, replay, and reports.
- [Project overview](https://github.com/ZippeyKeys12/austen): the Austen tools and crates.
