# `austen-lib`: Shared building blocks

The shared building blocks of [Austen](https://github.com/ZippeyKeys12/austen), a toolkit for C code with JSON specifications.
The generators, harness, CLI, and external-fuzzer adapters all use this crate.

It provides:

- The specification model, specification analysis, and generator language.
- Common input generation, campaign, coverage, and findings support.
- Native object and archive handling.
- Run events and JSON report types.

## Features

The specification and generator languages, object handling, and report types are available by default.
Additional functionality is optional:

| Feature | Purpose |
| --- | --- |
| `wire` | Communication between the driver and harness. |
| `driver` | Running campaigns and working with external fuzzers (includes `wire`). |
| `smt` | Z3 constraint solving and generator optimizations. |

Enable `smt` when solver support is needed.
It requires an installed Z3 library.

## Installation and documentation

To test C code with Austen, install the [austen CLI](https://crates.io/crates/austen) with the engine features you need and follow the [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it).

Add this crate with `cargo add austen-lib` only when building your own Rust tooling on Austen.
Select the optional Cargo features that tooling needs.
See the [build guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source) for the project overview and native build setup.
