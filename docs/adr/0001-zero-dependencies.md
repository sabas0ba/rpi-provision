# 0001. The workspace carries no external dependencies

Status: accepted

## Context

`rpi-provision` reads a TOML specification, renders text files and writes them
to a removable disk. The obvious dependency set would be `serde`, `toml` and
`clap`, which together pull in roughly a dozen transitive crates including two
proc-macro crates.

Three constraints pushed the other way:

1. The tool writes to a device that someone will then trust with SSH keys and
   a Wi-Fi pre-shared key. The supply chain that produces the binary is part
   of that trust.
2. It must be easy to build and distribute for Windows, Linux and both x86-64
   and aarch64. Fewer moving parts means fewer cross-compilation surprises.
3. The project convention is to pin dependencies to an exact version. Pinning
   a dozen transitive crates and keeping them current is real, recurring work
   for a tool whose problem domain barely changes.

## Decision

The workspace depends on `std` and nothing else. Specifically, the following
are implemented in-tree:

| Component | Crate | Size |
| --- | --- | --- |
| TOML subset parser | `rpi-provision-toml` | ~600 lines |
| SHA-256 | `rpi-provision-spec::sha256` | ~120 lines |
| Line diff (LCS) | `rpi-provision-apply::diff` | ~150 lines |
| Argument parser | `rpi-provision::args` | ~180 lines |

CI enforces this: a `Cargo.lock` containing any package that is not a
workspace member fails the build.

## Consequences

Accepted costs:

- The TOML parser covers a subset. Date-time values are rejected with an
  explicit diagnostic rather than silently mishandled. The subset is
  documented in the crate's module comment and covered by its own tests.
- Deserialisation is written by hand. This turned out to be an advantage:
  the reader rejects unknown keys and reports the source position of every
  error, which a derive-based approach would have needed extra configuration
  to match.
- Each in-tree component must be tested as thoroughly as the dependency it
  replaces. SHA-256 is checked against the FIPS 180-4 vectors; the parser and
  the diff have their own suites.

Benefits realised:

- `cargo build --release --locked` needs only a Rust toolchain, and the
  release binary is well under a megabyte.
- There is nothing to audit or update between releases.
- Cross-compiling to `aarch64-unknown-linux-musl` and
  `x86_64-pc-windows-msvc` needs no feature-flag archaeology.

## Alternatives considered

- **`serde` + `toml` + `clap`.** Conventional and less code to own, but three
  direct and about a dozen transitive dependencies for a tool that parses one
  file format and six subcommands.
- **Vendoring the dependencies.** Keeps the ergonomics but moves the audit
  burden in-tree without reducing it, and vendored proc-macro crates still
  need a build.

## Revisit if

The specification format grows to need full TOML (date-times, mixed-type
arrays), or the CLI grows subcommand hierarchies and shell completion. Neither
is on the roadmap.
