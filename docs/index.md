---
title: What it does
nav_id: home
---

# rpi-provision

Write a Raspberry Pi 5 SD card with Raspberry Pi Imager, point `rpi-provision`
at the FAT boot partition, and the first boot brings the board up fully
configured — account, SSH, networking, the USB gadget link and the peripheral
buses — with no keyboard, no monitor and no manual first login.

```console
$ export RPI_PASSWORD_HASH="$(openssl passwd -6)"
$ rpi-provision diff  pi.toml --boot /media/$USER/bootfs
$ rpi-provision apply pi.toml --boot /media/$USER/bootfs
```

The configuration is one TOML file. It is meant to live in version control, so
secrets are declared as a *source* rather than written into it.

## What it configures

| Area | Result |
| --- | --- |
| Account | User created, crypt(3) password hash, shell, supplementary groups, `sudo` rule |
| SSH | `authorized_keys`, an `sshd_config.d` drop-in, password authentication off by default, service enabled or disabled |
| Wired | NetworkManager keyfile, DHCP or static address, gateway, DNS, autoconnect priority |
| Wireless | NetworkManager keyfile, WPA2 or WPA3, hidden networks, regulatory domain |
| USB gadget | `dtoverlay=dwc2,dr_mode=peripheral`, a configfs composition script for ECM/NCM/RNDIS, a systemd unit and a static address on the link |
| Hardware | UART0 on GPIO 14/15, the dedicated debug UART, I²C with baud rate, SPI, 1-Wire, PCIe generation, arbitrary overlays and `dtparam`s |
| Localisation | Time zone, locale, keyboard map |
| Your own files | Any file or directory copied onto the root filesystem, with its mode and ownership |
| Your own commands | Shell commands run at the end of the first boot |

Every key, with its type, default and constraints, is in the
[specification reference](specification.md).

## Quick start

**1. Write the OS image.** Use Raspberry Pi Imager or `dd`. `rpi-provision`
does not download or write images, and it does not need Imager's own
customisation screen — leave it alone.

**2. Find the boot partition.** It is the small FAT one, usually mounted
automatically when the card is re-inserted.

```console
$ rpi-provision detect
/media/user/bootfs    Raspberry Pi 5
```

**3. Write a specification.** The smallest useful one is a host name, an
account and a key:

```toml
[meta]
schema_version = 1

[system]
hostname = "pi-minimal"

[user]
name = "engineer"
authorized_keys = [
  "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host",
]
```

**4. Look before you write.** `diff` prints exactly what `apply` would change
and touches nothing:

```console
$ rpi-provision diff pi.toml --boot /media/$USER/bootfs
```

**5. Apply.** The change set is printed and confirmed before anything is
written. `--backup` snapshots the whole partition first, so a bad card is a
`restore` away rather than a fresh run of Imager:

```console
$ rpi-provision apply pi.toml --boot /media/$USER/bootfs --backup ./before-apply
```

Eject the card, boot the board, and connect:

```console
$ ssh engineer@pi-minimal.local
```

Applying twice is a no-op, so a card can be re-provisioned as often as you
like. If something does go wrong, `revert` undoes what the tool added and
`restore` puts a whole snapshot back. The full command set is on the
[usage](usage.md) page, and there is a
[desktop application](gui.md) over the same operations if a window suits the
job better than a terminal.

## Examples

Two specifications ship with the repository, from the smallest thing that
works to a fully equipped bench board:

| Example | What it demonstrates |
| --- | --- |
| [`minimal.toml`](examples.md#minimal-key-only-access) | Key-only SSH access and nothing else — eleven lines |
| [`development.toml`](examples.md#development-a-fully-equipped-bench-board) | Static wired address with Wi-Fi fallback, a USB gadget link, I²C, SPI, UART and localisation |

Both are walked through, section by section, on the
[examples](examples.md) page, along with the pattern for driving a fleet of
boards from a single specification.

## What happens on the first boot

`apply` writes a payload to the boot partition and adds `systemd.run=` hooks
to `cmdline.txt`. On the first boot the runner:

1. copies the payload into `/run` (a tmpfs) and re-executes itself from there,
   so the secrets live in RAM and the FAT copy can be deleted while the script
   is still running;
2. runs `steps/*.sh` in numeric order — host name, account, payload install,
   SSH, network, USB gadget, localisation, and finally the commands declared
   in `[[run]]`;
3. removes its own hooks from `cmdline.txt`, so a failed run cannot become a
   boot loop;
4. deletes the payload from the boot partition;
5. records the outcome in `/var/lib/rpi-provision/status` and a transcript in
   `/var/log/rpi-provision.log`, then reboots into a normal boot.

Each step is a short, standalone POSIX shell script that you can read on the
card and run by hand. The details, including how to debug a run, are in
[first boot on the device](first-boot.md).

## Scope

- **Target.** Raspberry Pi 5 running Raspberry Pi OS Bookworm or later, which
  is to say a NetworkManager-based image. Model-specific behaviour that is
  easy to get wrong — the UART numbering, `dr_mode`, the USB-C power path — is
  collected in [Raspberry Pi 5 notes](raspberry-pi-5.md).
- **Writes.** The FAT boot partition, and nothing else. The ext4 root
  filesystem is never mounted, which is what puts Linux and
  [Windows](windows.md) on an equal footing and why no elevation, WSL or
  Docker is involved. The reasoning is in
  [ADR 0002](adr/0002-boot-partition-only.md).
- **Does not.** Download images, write cards, or install packages. Use
  Raspberry Pi Imager or `dd` for the first, and `apt` on the device for the
  last.

## Installation

CI publishes binaries for `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl` and `x86_64-pc-windows-msvc`. To build from
source:

```console
$ cargo build --release --locked
$ ./target/release/rpi-provision --help
```

The workspace has **no external dependencies**, so nothing but a Rust
toolchain is required — no `openssl-sys`, no vendored C, no network access
during the build. The reasoning is in
[ADR 0001](adr/0001-zero-dependencies.md).

## Security notes

- Until the first boot completes, the card's FAT partition holds the Wi-Fi
  pre-shared key and the password hash in plain text. FAT has no permissions,
  so anyone who can read the card can read them. The payload is deleted after
  a successful run, but deletion on FAT is not secure erasure.
- `rpi-provision` refuses to write to a directory that is not a Raspberry Pi 5
  boot partition unless `--allow-unverified-boot` is given.
- Diffs never print secret content.
- A snapshot taken with `backup` after an `apply` contains the payload, and so
  the Wi-Fi key and password hash, in an ordinary directory. Snapshot before
  applying, or treat the directory as sensitive.
