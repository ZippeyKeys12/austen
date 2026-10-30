# AustenTest guide

This guide accompanies the Austen source distribution. Run its commands from the repository root with `austen` on `PATH`.
Start with [installation](installation.md), or use the supplied Dockerfile as described there.

| Tutorial | What you will do |
| --- | --- |
| [Quickstart](quickstart/README.md) | Run supplied specifications against correct and buggy C, then replay a saved failure. |
| [A new target](new-target.md) | Export an annotated function and test independently compiled objects and archives. |
| [Heap ownership](ownership/README.md) | Free generated memory, track target allocations, and detect leaked ownership. |
| [Extend generation](extension/README.md) | Implement a producer using the existing reconstruction and execution interfaces. |
| [Limitations](limitations.md) | Understand supported platforms, inconclusive judgments, and replay requirements. |

The examples are small teaching targets, not the paper's evaluation workloads.
Their C sources and JSON specifications are included, so the quickstart and ownership examples do not require running CN.
The annotations are included to show the original specifications and support re-exporting them.
