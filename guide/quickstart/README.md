# Test and replay a C function

The supplied [specification](spec.json) says that `identity(x)` must return `x`.
[correct.c](correct.c) does so, while [buggy.c](buggy.c) deliberately returns zero for odd inputs.
Both have the same C signature and CN specification, so they use the same exported JSON.

Install Austen with Bennet, Darcy, and Wickham using the [installation guide](../installation.md).
Run the following commands from the repository root. The examples put generated files under `target/guide/`.

## Run a passing input

```sh
austen sanity guide/quickstart/spec.json
austen test guide/quickstart/spec.json guide/quickstart/correct.c \
  --input=0x00ffffffff07000000 \
  --work=target/guide/passing --report=target/guide/passing.json
```

This exact input selects the only function and supplies `x = 7`.
The report should have `dispatched: 1`, `crashes: 0`, and `findings.post_failed: 0`.
The sanity command reports no findings for this specification.
That is not a proof of completeness.

## Observe a postcondition failure

```sh
austen test guide/quickstart/spec.json guide/quickstart/buggy.c \
  --input=0x00ffffffff07000000 \
  --work=target/guide/buggy --report=target/guide/buggy.json
```

The target returns normally with zero, and the oracle detects that zero differs from seven.
The summary reports one crash attributed to a postcondition violation.
The JSON report has `dispatched: 1`, `crashes: 1`, and `findings.post_failed: 1`.
The failure is retained under `target/guide/buggy/crashes/postcondition-failing/`.

By default, finding a bug still exits successfully: the testing command itself completed.
Add `--fail-on-crash` when a finding should fail a CI job.
An unsupported oracle judgment is inconclusive, and must not be treated as a passing check.

## Replay the retained input

Each file in the findings directory contains the exact binary input, not a textual seed.
Replay the files saved by the preceding command:

```sh
for input in target/guide/buggy/crashes/postcondition-failing/*; do
  austen test guide/quickstart/spec.json guide/quickstart/buggy.c \
    --input=- --work=target/guide/replay < "$input"
done
```

Each replay should report the same postcondition violation.
Use the same specification, target, tool release, and harness settings.
Replay reads the function and size from the input, so omit generation flags such as `--engine`, `--seed`, and `--duration`.
See [replay limitations](../limitations.md) before sharing saved inputs across machines or builds.

## Generate inputs

To let a producer find the failure instead of supplying bytes:

```sh
austen test guide/quickstart/spec.json guide/quickstart/buggy.c \
  --engine=bennet --seed=7 --duration=2 \
  --work=target/guide/generated --report=target/guide/generated.json
```

Try `--engine=darcy`, `--engine=wickham`, or `--engine=bennet/darcy` with their enabled features.
The duration is the campaign budget.
Preparation and final analysis add time.
The number of executed inputs and retained findings depends on the machine and generator.
The exact-input examples above provide deterministic checks of the demonstrated verdicts.

## Specification provenance

These targets were written for this guide and are distributed under the repository's [MIT license](../../LICENSE).
`spec.json` was exported from `correct.c` using CN commit `720f556d821284afe67e0a4842def65ada74f255` from `rems-project/cn`, the revision selected by the Dockerfile.
It includes CN's exported logical declarations.
The [new-target tutorial](../new-target.md) gives the export command and prerequisites.
