# `austen-bennet`: Backtracking generation

Bennet generates test inputs for C functions from their JSON specifications.
This crate is Austen's Rust reimplementation of the original Bennet research tool.

When an input fails a constraint, Bennet goes back to the choices that could fix it and tries again, keeping the useful parts of the input.
It can also allocate the memory needed for structures such as linked lists.
This makes it more effective than restarting the whole input whenever a constraint fails.

## Use it

To use Bennet with Austen, install the `austen` CLI with the `bennet` feature:

```sh
cargo install austen --locked --features bennet
austen test spec.json target.c --engine=bennet --duration=30
```

Here, `spec.json` is the JSON specification for your `target.c`.
See the [build setup](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source) for prerequisites and the [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it) for specification export and campaigns.

Add this crate with `cargo add austen-bennet` only when building your own Rust tooling on the generator.

## How to cite

For the original research underlying this Rust reimplementation, cite:

Zain K Aamer and Benjamin C. Pierce.
2025.
**Bennet: Randomized Specification Testing for Heap-Manipulating Programs.**
*Proceedings of the ACM on Programming Languages* 9, OOPSLA2, Article 413 (October 2025).
[DOI: 10.1145/3764115](https://doi.org/10.1145/3764115).
[Author copy](https://zain-k-aamer.com/papers/bennet.pdf).

```bibtex
@article{aamer2025bennet,
  author = {Aamer, Zain K and Pierce, Benjamin C.},
  title = {{Bennet}: Randomized Specification Testing for
           Heap-Manipulating Programs},
  journal = {Proceedings of the ACM on Programming Languages},
  volume = {9},
  number = {OOPSLA2},
  articleno = {413},
  year = {2025},
  month = oct,
  doi = {10.1145/3764115}
}
```
