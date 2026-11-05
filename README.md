# cs2-dumper

An external offset/interface dumper for the Windows version of Counter-Strike 2, runnable on Windows or Linux hosts with an appropriate connector. Powered
by [memflow](https://github.com/memflow/memflow).

The native Linux version is available in the [linux](https://github.com/a2x/cs2-dumper/tree/linux) branch (currently
outdated).

For a work-in-progress offline version, check out the [cs2-analyzer](https://github.com/a2x/cs2-analyzer) repository or
view its included web demo [here](https://a2x.github.io/cs2-analyzer).

## Getting Started

You can download the latest release from [Releases](https://github.com/a2x/cs2-dumper/releases) or compile it yourself.
Use a current stable 64-bit Rust toolchain. The Rust 2024 edition requires at least Rust 1.85; dependencies may require a newer compiler.

## Usage

1. Ensure the game is running (Being in the main menu should suffice).
2. Run the `cs2-dumper` executable.

_Note:_ On Windows, if you run the executable without specifying an optional memflow connector name, it will automatically use the
[memflow-native](https://github.com/memflow/memflow-native) OS layer to read the memory of the game process. If you
wish to use an existing memflow connector instead, such as **pcileech** or **kvm**, you can pass the `connector` and
optional `connector-args` arguments to the program. These connectors can be installed and managed using
the [memflowup](https://github.com/memflow/memflowup) tool.

On Linux hosts, `--connector` is required and the target must be Windows. This branch does not analyze native Linux CS2 binaries. `--connector-args` requires `--connector`.

E.g (for pcileech). `cs2-dumper -c pcileech -a :device=FPGA -vv`

Certain connectors, such as the [kvm](https://github.com/memflow/memflow-kvm) connector on Linux or
the [pcileech](https://github.com/memflow/memflow-pcileech) / [winio](https://github.com/a2x/memflow-winio)
connectors on Windows, require elevated privileges to work. So either run the `cs2-dumper` executable with `sudo` on
Linux or as an administrator on Windows.

### Available Arguments

- `-c, --connector <connector>`: The name of the memflow connector to use.
- `-a, --connector-args <connector-args>`: Additional arguments to pass to the memflow connector.
- `-f, --file-types <file-types>`: The types of files to generate. Default: `cs`, `hpp`,  `json`, `rs`, `zig`.
- `-i, --indent-size <indent-size>`: The number of spaces to use per indentation level. Default: `4`; allowed range: `0`–`16`.
- `-o, --output <output>`: The output directory to write the generated files to. Default: `output`.
- `-p, --process-name <process-name>`: The name of the game process. Default: `cs2.exe`.
- `-v...`: Increase logging verbosity. Can be specified multiple times.
- `-n, --no-log-file`: Do not create `cs2-dumper.log`.
- `-h, --help`: Print help.
- `-V, --version`: Print version.

## Running Tests

Run the offline unit tests (no game or connector required):

```sh
cargo test
```

The Windows live-process tests are ignored by default. After generating fresh output with CS2 running, run them explicitly:

```sh
cargo test -- --ignored --nocapture
```

These live tests may require being connected to a map and sufficient permissions to read the game process.

## Failure handling

Invalid arguments are rejected before analysis. Incomplete fixed-size reads, detected list cycles, invalid counts, and missing offset patterns return errors rather than silently publishing an incomplete successful dump. All files are rendered and the build number is read before existing output is changed. `info.json` is written last as the completion marker; a missing or invalid marker means publication did not complete. Publishing the entire directory is not transactional, so avoid reading it while a dump is running. Files for modules removed in a newer game build are not automatically deleted; use a fresh output directory when changing builds.

### Optional Steam interface module

`vstdlib_s64.dll` is a Steam support module, not a required Source 2 game module. Its interface implementation may differ from the layout this dumper understands. If reading its image, resolving its `CreateInterface` export, or walking its interface list fails, the dumper logs a warning with the module name and full error chain and continues. All interfaces from that failed module are omitted; partial entries are not published. Successful reads are still retained.

This exception is limited to `vstdlib_s64.dll` (case-insensitive). Errors in other modules, including `client.dll`, `engine2.dll`, and `schemasystem.dll`, remain fatal. Finding no interfaces at all also remains an error. Check the log for warnings before treating an interface inventory as complete. This change does not add support for a different Steam interface layout or update game offset signatures.

### Interface factory decoding

Interface output contains **instance RVAs relative to the registering module**, not arbitrary instruction operands or absolute heap addresses. The dumper recognizes small x64 `lea rax, [rip+disp32]; ret` and `mov rax, [rip+disp32]; ret` factories, null-return stubs, and bounded direct/indirect jump thunks (including NOP/ENDBR64 prefixes). A `mov` factory is dereferenced; its pointer slot is not mislabeled as the instance.

Null-returning factories, unsupported factory implementations, and instances outside the registering module are omitted **individually**, with a warning naming the interface, module, and factory address. Other entries in that module are still processed. This avoids treating arbitrary code bytes as a displacement and failing the whole dump with `interfaces address outside module`. It does not guarantee a complete interface inventory: check the warnings for omitted entries. Memory read failures, truncated recognized instructions, cycles, and traversal-limit failures still propagate as errors (subject to the optional Steam-module exception above). Game offset and schema failures are not suppressed.

If you see `outdated pattern`, the executable signatures or memory layouts may need updating for that particular CS2 build. Run with `-vvv` and keep the full error message and game build number; do not reuse old offsets as if they were freshly extracted.

## License

Licensed under the MIT license ([LICENSE](./LICENSE)).
