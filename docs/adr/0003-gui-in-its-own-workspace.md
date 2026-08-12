---
title: ADR 0003 · GUI in its own workspace
nav_id: adr
---

# 0003. The desktop front end lives in a separate workspace

## Context

[ADR 0001](0001-zero-dependencies.md) says the workspace carries no external
dependencies, and CI enforces it by checking that `Cargo.lock` never lists a
package that is not a workspace member. That property is what makes the
library and the command line buildable anywhere with nothing but a Rust
toolchain — no `openssl-sys`, no vendored C, no network during the build.

A desktop application cannot be written that way. Drawing a window means a
toolkit, and every candidate brings hundreds of transitive crates:

- **Tauri** renders in the platform's own webview. It brings the largest
  dependency tree of the three, needs WebKitGTK development packages on
  Linux, and is the only option here that does not open a network port.
- **A local web UI** — an HTTP server in `std` serving a page to the
  operator's browser — would have kept the dependency count at zero. It was
  rejected: a listening socket on loopback is reachable by every process and
  every user on the machine, and a browser tab on another site can be made to
  talk to it. A tool whose job is writing credentials onto a card should not
  open a port to do it.
- **A native toolkit** (egui, GTK) is comparable in dependency count to Tauri
  without removing the need for system libraries.

## Decision

The application lives in `gui/`, which is a Cargo workspace of its own,
excluded from the root workspace. It depends on `crates/spec`, `crates/render`
and `crates/apply` by path, and on Tauri from crates.io.

Consequently:

- the root `Cargo.lock` still lists only workspace members, and the CI check
  that enforces ADR 0001 is unchanged;
- `cargo build`, `cargo test` and `cargo clippy` at the root neither build nor
  need any of Tauri;
- the library and the command line remain buildable with no network and no
  system packages;
- `gui/` has its own lock file and its own CI job.

The dependency arrow only ever points inwards. Nothing under `crates/` may
depend on anything in `gui/`, and no type crosses the boundary that the
command line does not already use.

## Consequences

- Two workspaces means two lock files, two `cargo test` invocations and a CI
  job that installs WebKitGTK. The root workspace's build stays fast; the
  GUI's does not.
- The window is not the product. Anything the GUI can do, the command line can
  do, because the GUI calls the same three crates — it holds no logic of its
  own beyond editing a TOML document and shaping results for display. A
  feature added to the GUI alone would be a bug.
- Building the GUI needs system packages that the rest of the project does
  not: `libwebkit2gtk-4.1-dev` and `libgtk-3-dev` on Debian and Ubuntu,
  WebView2 on Windows, nothing extra on macOS. Somebody who only wants the
  CLI never encounters them.
- The GUI's dependencies need the attention that ADR 0001 was written to
  avoid: they have their own advisories and their own upgrade churn. That cost
  is now confined to one directory that nothing else builds.

## Alternatives considered

**Relax ADR 0001 and put the GUI in the main workspace.** The simplest
arrangement, and it would have cost the property that makes the tool easy to
build and audit. The reason for ADR 0001 does not weaken because a second
front end exists.

**Ship the GUI as a separate repository.** It would keep this one clean, at
the price of version skew between the front end and the crates it drives, and
a second place to make every change. The boundary needed is between dependency
trees, not between repositories.

**A terminal UI instead.** No dependencies, works over SSH — and not what was
asked for. It remains available as an addition rather than as a substitute.

## Revisit if

- the platform webview stops being available on a target that matters, which
  would make Tauri the wrong choice rather than the boundary wrong;
- the GUI grows logic that the command line does not have, at which point that
  logic belongs in `crates/` and this ADR is not what is wrong;
- Cargo grows a way to isolate a dependency tree within one workspace, which
  would make the split unnecessary.
