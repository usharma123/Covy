# Release package verification

`python3 scripts/verify_release_packages.py source` is the pre-tag npm release
check. It renders the root package and all four platform package manifests in a
temporary directory, then runs both `npm pack --dry-run` and
`npm publish --dry-run` with npm's offline mode enabled. The command cannot
write to the registry and is part of the canonical full gate.

The tagged release validates the exact staged package before each platform
artifact is uploaded. The verifier checks:

- the package name, version, `os`, `cpu`, file allowlist, and executable modes;
- all three expected binaries and no unexpected files or symlinks;
- a thin Mach-O or 64-bit ELF header with the matrix target architecture;
- `Packet28 --version`, `packet28d --version`, and `p28 --help`; and
- the offline npm pack/publish dry-run output and generated package integrity.

Execution coverage follows the available runner boundary:

| Package | Execution |
| --- | --- |
| `darwin-arm64` | Native on the ARM64 macOS runner. |
| `darwin-x64` | Native when the runner is x86_64; otherwise header and npm metadata only. The ARM64-hosted job does not assume Rosetta, so Intel execution remains an explicit external release check. |
| `linux-x64` | Native on the x86_64 Linux runner. |
| `linux-arm64` | Executed through `qemu-aarch64`; the shipped musl binaries are static. |

The publish job repeats the offline npm verification after downloading the
artifacts. Platform packages travel inside tar archives because GitHub artifact
downloads do not preserve Unix executable modes. The publish job extracts each
archive, rechecks its binary headers and modes, executes the native Linux x64
artifact, and repeats the npm dry-run. This catches transfer or assembly drift
before publication without weakening the separate post-publication install
smoke recorded by the release experiment.
