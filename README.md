# LaplacianOS

LaplacianOS is a Rust operating system with a capability-oriented kernel, native
typed temporal graph, predictive operating-system control, and a built-in SCCE
cognitive runtime. It boots through a custom UEFI loader and runs native ring-3
services and applications on a GPU-composited desktop.

## Architecture

| Layer | Responsibility |
| --- | --- |
| UEFI loader | Loads the kernel and transfers typed boot information. |
| Kernel | Scheduling, memory, IPC, capabilities, drivers, bounded graph mutation, and telemetry. |
| Graph runtime | Typed nodes and edges, temporal attenuation, deterministic PowerWalk, provenance, causal evidence, spectral state, and predictive telemetry. |
| Ring-3 services | `graphd`, `modeld`, `trainerd`, `artifactsd`, `sysd`, `servicemgr`, compositor, networking, and supporting services. |
| SCCE runtime | Document ingestion, retrieval, entity correlation, graph reasoning, conversation state, tool planning, and auditable outputs. |
| Applications | AI Console, Predictive System Monitor surfaces, Terminal, Files, Settings, Editor, Browser Lite, and 3D applications. |
| SDKs and tools | Application, UI, GL, graph, tool, WASM, packaging, installation, and verification tooling. |

LaplacianOS has two distinct intelligence domains that share the graph, provenance,
event, and capability substrate:

- Operating-system intelligence processes live telemetry to forecast system
  state and propose bounded, reversible actions.
- SCCE cognition processes documents, entities, relations, conversations, and
  tool results for local retrieval, reasoning, synthesis, and tool use.

Heavy corpus processing, long-running prediction, and model orchestration run
in ring 3. Kernel paths remain bounded and must not block on prediction.

## Repository layout

- `kernel/`: kernel, graph substrate, cognitive bootstrap, and hardware/runtime integration.
- `boot/`: UEFI boot loader.
- `userspace/`: native services, compositor, shell, and protected ring-3 runtime.
- `apps/`: native applications.
- `sdk/`: application, UI, GL, graph, tool, WASM, and hashing SDKs.
- `tools/`: package, image, benchmark, cryptographic, and verification tools.
- `scripts/`: build, QEMU, release, and verification entry points.
- `docs/`: architecture, ABI, operational, and validation documentation.

## Build requirements

- Rust nightly as pinned by `rust-toolchain.toml`.
- `rust-src`, `llvm-tools-preview`, `rustfmt`, and `clippy`.
- QEMU and OVMF for x86_64 system testing.
- PowerShell 7+ for the Windows build and verification scripts.

## Quick checks

```powershell
cargo check -p laplacianos-kernel
cargo check -p laplacianos-trainerd
cargo check -p laplacianos-artifactsd
cargo check -p laplacianos-sysd
cargo check -p laplacianos-ai-console
cargo check -p laplacianos-appstore
```

For the repository verification gate:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\verify.ps1 -SkipBoot
```

To boot the x86_64 system in QEMU:

```powershell
.\scripts\boot-windows-qemu.ps1
```

## Mathematics

The temporal graph and predictive runtime use explicit, testable operators with
documented units and assumptions. The name Walsh does not imply a
Walsh-Hadamard transform. LaplacianOS does not use the removed type-space FWHT
interpretation. Authoritative mathematical implementations must be traceable to
their source equations and independent numerical validation.

## License

LaplacianOS is proprietary and source-available for inspection only. No
license is granted to use, copy, modify, distribute, or create derivative
works from this software except under a separate written agreement with
Ryan P. Walsh.

Licensing inquiries may be made through
[LinkedIn](https://www.linkedin.com/in/uiarchitect/) or the
[LaplacianOS GitHub issue tracker](https://github.com/rpwalsh/LaplacianOS/issues).
Contacting the licensor does not itself grant permission; rights are granted
only through a separate executed written agreement.

See `LICENSE` and `NOTICE`.
