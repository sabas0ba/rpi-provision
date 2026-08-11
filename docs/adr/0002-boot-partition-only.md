# 0002. Write only to the FAT boot partition

Status: accepted

## Context

A Raspberry Pi SD card has two partitions: a FAT32 boot partition and an ext4
root filesystem. Configuring a headless device means changing things in both
places — `config.txt` and `cmdline.txt` live on the first,
`/etc/NetworkManager/system-connections/`, `/etc/passwd` and
`/etc/ssh/sshd_config.d/` on the second.

The tool has to run on Windows as well as Linux. That asymmetry dominates the
design:

| Operation | Linux | Windows |
| --- | --- | --- |
| Read and write FAT32 | native | native (drive letter) |
| Read and write ext4 | native | needs `wsl --mount`, or a third-party driver |
| Loop-mount a `.img` | `losetup` | needs WSL2 or a privileged container |

Reaching the root filesystem from Windows means requiring WSL2, elevation and
a physical-disk mount — a large step up in prerequisites and in the blast
radius of a mistake.

## Decision

`rpi-provision` writes only to the FAT boot partition. Everything that belongs
on the root filesystem is staged as a payload plus a manifest and installed
during the first boot by a generated runner, hooked in through `systemd.run=`
on the kernel command line.

## Consequences

- The Windows and Linux code paths are identical. There is no `#[cfg]` in the
  writing path beyond setting the executable bit, which FAT ignores anyway.
- No elevated privileges, no WSL, no Docker, no loop devices.
- The tool is a single self-contained binary that operates on a directory.
  That makes the whole thing testable against an in-memory filesystem, and it
  is why `apply` and its dry run share one code path.

Accepted costs:

- **One extra boot.** The device boots, applies the configuration and reboots.
  Nothing is configured until the card has been in a Pi once.
- **Secrets sit on FAT until then.** The Wi-Fi pre-shared key and the password
  hash are readable by anyone who can read the card. The runner deletes the
  payload after a successful run, but deletion on FAT is not secure erasure.
  This is called out in the README and in `apply`'s output.
- **No package installation.** Anything requiring `apt` needs a network and
  therefore belongs to a later stage; this tool does not attempt it.
- **The device does the work, so the device can fail.** Diagnosis needs the
  log on the card rather than an error at the desk. Mitigated by validating
  everything that can be validated up front, by keeping each step small and
  independently runnable, and by writing an explicit status file.

## Alternatives considered

- **Mount the ext4 root filesystem and write directly.** Removes the extra
  boot and the secrets-on-FAT window. Rejected because it makes Windows a
  second-class platform: `wsl --mount \\.\PHYSICALDRIVE<n>` needs
  administrator rights and WSL2, and a bug there can damage a partition table
  rather than one file.
- **Customise a `.img` before writing it.** Best reproducibility, and the
  right answer for a build pipeline. Rejected as the primary mode because it
  requires loop mounting, hence a Linux host or a privileged container, and
  because it cannot fix a card that has already been written.
- **Emit `custom.toml` for the stock Raspberry Pi Imager mechanism.** Covers
  the account, SSH and Wi-Fi, but nothing about UART, GPIO or USB gadget mode,
  so `config.txt` and a private runner would be needed anyway. Running two
  first-boot mechanisms leaves their relative order undefined; `apply` refuses
  to proceed when it finds one of the other mechanism's files.

## Revisit if

An `.img` customisation mode is wanted for CI. That would be an additional
backend behind the same `Plan`, not a replacement: `render` is already
independent of the filesystem, and `BootFs` is already a trait.
