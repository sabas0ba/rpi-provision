# rpi-provision

Declarative first-boot provisioning for Raspberry Pi 5 SD cards.

Write the card with Raspberry Pi Imager (or `dd`), point `rpi-provision` at the
FAT boot partition, and the first boot configures SSH, networking, the USB
gadget link and the peripheral buses without a keyboard or monitor.

```console
$ export RPI_PASSWORD_HASH="$(openssl passwd -6)"
$ rpi-provision diff examples/development.toml --boot /media/$USER/bootfs
$ rpi-provision apply examples/development.toml --boot /media/$USER/bootfs
```

## Scope

- **Target**: Raspberry Pi 5 running Raspberry Pi OS (Bookworm or later, i.e.
  NetworkManager based).
- **Writes**: the FAT boot partition only. The ext4 root filesystem is never
  mounted, which is what keeps Linux and Windows on an equal footing.
- **Does not**: download images, write cards, or install packages. Use
  Raspberry Pi Imager or `dd` for the first, `apt` on the device for the last.

## What it configures

| Area | Result |
| --- | --- |
| Account | User created, crypt(3) password hash, shell, supplementary groups, `sudo` rule |
| SSH | `authorized_keys`, `sshd_config.d` drop-in, password authentication off by default, service enabled or disabled |
| Wired | NetworkManager keyfile, DHCP or static address, gateway, DNS, autoconnect priority |
| Wireless | NetworkManager keyfile, WPA2 or WPA3, hidden networks, regulatory domain |
| USB gadget | `dtoverlay=dwc2,dr_mode=peripheral`, a configfs composition script for ECM/NCM/RNDIS, a systemd unit, and a static address on the link |
| Hardware | UART0 on GPIO 14/15, the dedicated debug UART, I²C with baud rate, SPI, 1-Wire, PCIe generation, arbitrary overlays and `dtparam`s |
| Localisation | Time zone, locale, keyboard map |

## Installation

Prebuilt binaries are published by CI for `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl` and `x86_64-pc-windows-msvc`. To build from
source:

```console
$ cargo build --release --locked
$ ./target/release/rpi-provision --help
```

The workspace has **no external dependencies**, so the build needs nothing but
a Rust toolchain. See `docs/adr/0001-zero-dependencies.md`.

## Usage

```
rpi-provision validate <SPEC>              Parse and validate a specification
rpi-provision render   <SPEC> --out DIR    Write the generated files to a directory
rpi-provision diff     <SPEC> --boot PATH  Show what apply would change
rpi-provision apply    <SPEC> --boot PATH  Write the provisioning payload to a card
rpi-provision revert   <SPEC> --boot PATH  Undo a previous apply
rpi-provision detect                       List mounted Raspberry Pi boot partitions
```

`apply` prints the change set and asks for confirmation. Without a terminal on
standard input it refuses to proceed unless `--yes` is given, so an
unattended script has to say so explicitly.

Applying twice is a no-op: the `config.txt` managed block is replaced rather
than appended, `cmdline.txt` is edited token by token, and every generated
file is compared before being written.

### Runtime parameter injection

A specification file is meant to live under version control, so secrets are
declared as a *source*, never as a literal:

```toml
[user]
password_hash = { env = "RPI_PASSWORD_HASH" }   # from the environment
# password_hash = { file = "secrets/pw.hash" }  # from a file, relative to the spec
# password_hash = { value = "$6$..." }          # inline, only if you mean it
```

A bare string in a secret field is rejected.

Anything can also be overridden on the command line, which is how one
specification serves a fleet:

```console
$ rpi-provision apply fleet.toml --boot /media/$USER/bootfs \
    --set system.hostname=dev-pi-07 \
    --set 'network.ethernet[0].address=192.168.1.57/24' \
    --set-secret user.password_hash=env:PI07_HASH
```

`--set` values appear in the process list; use `--set-secret` (which takes
`env:NAME`, `file:PATH` or `value:LITERAL`) for anything confidential.

### Windows

The boot partition is FAT32, so Windows mounts it with a drive letter and no
Docker, WSL or ext4 driver is involved:

```console
> rpi-provision detect
D:\    Raspberry Pi 5
> rpi-provision apply pi.toml --boot D:\
```

Generated files always use LF line endings, which the Raspberry Pi firmware
and `/bin/sh` both require. See `docs/windows.md`.

## What happens on the device

`apply` adds `systemd.run=` hooks to `cmdline.txt`. On the first boot systemd
runs `/boot/firmware/rpi-provision/firstrun.sh`, which:

1. copies the payload into `/run` (a tmpfs) and re-executes itself from there,
   so secrets never linger and the FAT copy can be deleted safely;
2. runs `steps/*.sh` in numeric order — host name, account, payload install,
   SSH, network, USB gadget, localisation;
3. removes its own hooks from `cmdline.txt`, so a failed run cannot become a
   boot loop;
4. deletes the payload from the boot partition (unless
   `provisioning.wipe_payload = false`);
5. writes the outcome to `/var/lib/rpi-provision/status` and a transcript to
   `/var/log/rpi-provision.log`, then reboots into a normal boot.

Details in `docs/first-boot.md`.

## Security notes

- Until the first boot completes, the card's FAT partition holds the Wi-Fi
  pre-shared key and the password hash in plain text. FAT has no permissions,
  so anyone who can read the card can read them. The payload is deleted after
  a successful run, but deletion on FAT is not secure erasure.
- `rpi-provision` refuses to write to a directory that is not a Raspberry Pi 5
  boot partition unless `--allow-unverified-boot` is given.
- Diffs never print secret content.

## Documentation

| File | Contents |
| --- | --- |
| `docs/specification.md` | Every key, its type, default and constraints |
| `docs/first-boot.md` | What runs on the device, and how to debug it |
| `docs/raspberry-pi-5.md` | Model specific behaviour and its sources |
| `docs/windows.md` | Using the tool from Windows |
| `docs/adr/` | Architecture decision records |

## Development

```console
$ cargo test
$ cargo clippy --all-targets -- -D warnings
$ cargo fmt --all -- --check
```

`crates/cli/tests/generated_shell.rs` renders the example specifications and
checks the resulting scripts with both `dash` and `bash`, exercises the
payload installer, and asserts that secret material appears in exactly the
two files that need it. CI additionally runs `shellcheck`.

## Licence

MIT OR Apache-2.0.
