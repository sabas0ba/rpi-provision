---
title: Desktop application
nav_id: gui
---

# The desktop application

A window over the same three crates the command line drives, for the times
when reading a diff in a terminal is not what you want. Everything it can do,
`rpi-provision` can do; it holds no logic of its own.

Every [tagged release](https://github.com/sabas0ba/rpi-provision/releases)
carries it as a `.deb` for Debian and Ubuntu and an `.msi` for Windows:

```console
$ sudo apt install ./rpi-provision-gui-v1.0.0-deb.deb
$ rpi-provision-gui
```

The package depends on `libwebkit2gtk-4.1-0` and `libgtk-3-0`, so `apt` pulls
the webview in rather than leaving you with something that installs and then
will not start. Or build it yourself:

```console
$ cargo build --release --manifest-path gui/Cargo.toml
$ ./gui/target/release/rpi-provision-gui
```

## What it is for

- **Finding the card.** Detect lists mounted boot partitions with the model
  read from the firmware blob, and fills in the path.
- **Editing a specification.** A form for the settings that are single values,
  and the document itself on the TOML tab. Both edit the same text.
- **Supplying secrets.** The specification names environment variables; the
  Secrets panel gives them values for this window only. They are never written
  to the file, and the real environment is used for any left blank.
- **Seeing what would change.** Preview prints the same change set `diff`
  does, secrets withheld the same way.
- **Writing, undoing and snapshotting.** Apply, Revert, and the snapshot
  operations, with the same confirmation and the same refusal to touch a
  directory that is not a boot partition.

## The document is the source of truth

The form does not keep a model of its own. Each control edits the TOML through
a format-preserving parser, so **comments and layout survive being edited** —
a specification is meant to stay in version control, and a tool that reformats
it on every change would make that miserable.

That is also why clearing a field removes the key rather than writing an empty
string: what applies afterwards is the default in the
[reference](specification.md), not a blank.

Lists — `[[network.ethernet]]`, `[[network.wifi]]`, `[[files]]`, `[[run]]` —
are edited on the TOML tab. The form covers the settings that are single
values, and the tab covers the rest of the file; validation runs over the
whole thing either way.

## Validation is the real validator

The status panel is `rpi-provision validate` running on every change, with the
same messages, the same line and column numbers, and the same warnings. There
is no second implementation to disagree with the first — if the window says a
specification is good, the command line will too, and the digest shown is the
one that ends up on the card.

## Building it

The GUI is a Cargo workspace of its own, deliberately: it depends on Tauri,
and the crates it drives depend on nothing at all. The reasoning is in
[ADR 0003](adr/0003-gui-in-its-own-workspace.md).

It therefore needs system packages that the rest of the project does not:

| Platform | What is needed |
| --- | --- |
| Debian, Ubuntu | `libwebkit2gtk-4.1-dev`, `libgtk-3-dev` |
| Windows | WebView2, which Windows 11 already has |
| macOS | Nothing beyond the Xcode command line tools |

Nothing about the command line changes: `cargo build` at the root still needs
only a Rust toolchain, and never builds any of the above.

There is no macOS package yet. The application builds there — Tauri uses the
system webview and needs nothing extra — but nothing in the release workflow
produces or tests a `.dmg`, so shipping one would be a claim nobody has
checked.

## What it deliberately does not do

- **No file picker.** Paths are typed. A dialogue would mean granting the
  window a filesystem plugin, and the operations that matter already take a
  path.
- **No network.** The window talks to the commands in the application binary
  and to nothing else. There is no port open on the machine, which is the
  reason it is a native window rather than a page served to your browser.
- **No unattended mode.** Automation belongs to the command line, which has
  `--yes` and exit statuses. See [usage](usage.md).
