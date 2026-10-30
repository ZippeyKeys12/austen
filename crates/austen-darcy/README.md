# `austen-darcy`: SMT generation

Darcy generates test inputs for C functions from their JSON specifications using the Z3 constraint solver.
This crate is Austen's Rust reimplementation of the original Darcy research tool.

Darcy first tries random values.
If they do not satisfy the specification, it asks Z3 for values that do, while preferring to keep the original choices.
This helps with restrictive conditions that random sampling would rarely satisfy.
The resulting inputs are checked before they reach the target C function.

## Use it

Darcy requires the native Z3 5.1.0 library with matching `pkg-config` metadata.
Follow the [build setup](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source) first.

To use Darcy with Austen, install the `austen` CLI with the `darcy` feature:

```sh
cargo install austen --locked --features darcy
austen test spec.json target.c --engine=darcy --duration=30
```

Here, `spec.json` is the JSON specification for your `target.c`.
See the [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it) for specification export and campaigns.

Add this crate with `cargo add austen-darcy` only when building your own Rust tooling on the generator.

## How to cite

For the original research introducing Darcy, described in Section 7, cite:

Zain K Aamer, Rini Banerjee, Hiroyuki Katsura, David Kaloper-Meršinjak, Dimitrios J. Economou, Kayvan Memarian, Dhruv Makwana, Neel Krishnaswami, Benjamin C. Pierce, Christopher Pulte, and Peter Sewell.
2026.
**Code-Specify-Test-Debug-Prove: Flexibly Integrating Separation Logic Specification into Conventional Workflows.**
*Proceedings of the ACM on Programming Languages* 10, PLDI, Article 200 (June 2026).
[DOI: 10.1145/3808278](https://doi.org/10.1145/3808278).
[Author copy with supplementary material](https://www.cl.cam.ac.uk/~pes20/pldi2026-paper155-supplementary.pdf).

```bibtex
@article{aamer2026darcy,
  author = {Aamer, Zain K and Banerjee, Rini and Katsura, Hiroyuki and
            Kaloper-Mer{\v{s}}injak, David and Economou, Dimitrios J. and
            Memarian, Kayvan and Makwana, Dhruv and Krishnaswami, Neel and
            Pierce, Benjamin C. and Pulte, Christopher and Sewell, Peter},
  title = {Code-Specify-Test-Debug-Prove: Flexibly Integrating Separation
           Logic Specification into Conventional Workflows},
  journal = {Proceedings of the ACM on Programming Languages},
  volume = {10},
  number = {PLDI},
  articleno = {200},
  year = {2026},
  month = jun,
  doi = {10.1145/3808278}
}
```
