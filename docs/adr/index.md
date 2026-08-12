---
title: Design decisions
nav_id: adr
---

# Design decisions

Three choices shape everything else in this tool, and each of them cost
something. They are recorded here with the context that produced them, so that
a later reader can tell whether the reasoning still holds.

| Record | Decision | Why it matters |
| --- | --- | --- |
| [0001](0001-zero-dependencies.md) | The workspace carries no external dependencies | The build needs nothing but a Rust toolchain — no `openssl-sys`, no vendored C, no network. The cost is a hand-written TOML parser, SHA-256, line diff and argument parser |
| [0002](0002-boot-partition-only.md) | Write only to the FAT boot partition | The ext4 root filesystem is never mounted, so Linux and Windows are equal footing and nothing needs elevation. The cost is that everything has to be deferred to a first-boot script |
| [0003](0003-gui-in-its-own-workspace.md) | The desktop application is a workspace of its own | A window needs a toolkit, and a toolkit needs hundreds of crates. Keeping it out of the main workspace is what lets 0001 stand. The cost is two lock files and a second CI job |

Each record follows the same shape: context, decision, consequences,
alternatives considered, and the conditions under which it should be
revisited.
