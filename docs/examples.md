---
title: Examples
nav_id: examples
---

# Examples

Both specifications below ship with the repository, under
[`examples/`](https://github.com/sabas0ba/rpi-provision/tree/main/examples),
and both are rendered and checked by CI — so what is on this page is what the
tool actually produces.

## Minimal: key-only access

Enough to get a board onto the network and accept your SSH key. No password
exists, so the account is locked to key authentication.

```toml
[meta]
schema_version = 1
target = "pi5"

[system]
hostname = "pi-minimal"

[user]
name = "engineer"
authorized_keys = [
  "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKeyBodyThatIsLongEnough user@host",
]
```

```console
$ rpi-provision apply examples/minimal.toml --boot /media/$USER/bootfs
```

What that produces on the card:

- a managed block at the end of `config.txt` and the first-boot hooks in
  `cmdline.txt`;
- four step scripts — host name, account, payload install, SSH;
- `authorized_keys`, installed into the account's `~/.ssh` with the right
  ownership on the device.

Wired networking needs no configuration at all: DHCP is what Raspberry Pi OS
already does. SSH is enabled by default, and password authentication is off by
default — which is why at least one key is required here.

## Development: a fully equipped bench board

A static wired address with Wi-Fi as a fallback, a USB gadget link so the
board is reachable over a single USB-C cable, the peripheral buses on, and
Japanese localisation.

```toml
[meta]
schema_version = 1
target = "pi5"
description = "Bench development board"

[system]
hostname = "dev-pi-01"
timezone = "Asia/Tokyo"
locale = "en_US.UTF-8"
keymap = "jp"

[user]
name = "engineer"
password_hash = { env = "RPI_PASSWORD_HASH" }
authorized_keys_files = ["authorized_keys"]
groups = ["gpio", "i2c", "spi", "dialout", "plugdev"]
sudo = "nopasswd"
shell = "/bin/bash"

[ssh]
enabled = true
password_authentication = false
permit_root_login = "no"
port = 22

[network]
wifi_country = "JP"

[[network.ethernet]]
id = "eth0-static"
interface = "eth0"
method = "manual"
address = "192.168.1.50/24"
gateway = "192.168.1.1"
dns = ["192.168.1.1", "1.1.1.1"]
autoconnect_priority = 100

[[network.wifi]]
id = "home"
ssid = "MySSID"
psk = { env = "WIFI_PSK_HOME" }
autoconnect_priority = 50

[network.usb_gadget]
enabled = true
function = "ecm"
interface = "usb0"
address = "10.55.0.1/24"
peer_address = "10.55.0.2"

[hardware.uart]
enabled = true
console = false

[hardware.i2c]
enabled = true
baudrate = 400000

[hardware.spi]
enabled = true

[hardware]
pcie_gen = 3
overlays = ["disable-bt"]

[[files]]
source = "files/motd"
destination = "/etc/motd"

[[files]]
source = "files/sysctl.d"
destination = "/etc/sysctl.d"
mode = "0644"

[[run]]
description = "refresh the package index"
command = "apt-get update"
ignore_failure = true
```

Because both secrets come from the environment, the file above is safe to
commit:

```console
$ export RPI_PASSWORD_HASH="$(openssl passwd -6)"
$ export WIFI_PSK_HOME="..."
$ rpi-provision apply examples/development.toml --boot /media/$USER/bootfs
```

### What the interesting parts do

**Two connections, one priority order.** `autoconnect_priority` decides which
NetworkManager profile wins when both are available: the wired link at 100,
Wi-Fi at 50. A statically addressed wired connection is written with
`may-fail=false`, so `network-online.target` waits for it; everything else
uses `may-fail=true` so an absent network never delays boot.

**`wifi_country` is not optional.** Any `[[network.wifi]]` requires it. Without
a regulatory domain the radio stays soft-blocked and the board simply never
associates.

**The USB gadget link.** The board takes `10.55.0.1` on the USB-C port, so it
is reachable over the same cable that would otherwise only power it. Two Pi 5
specifics apply: `dr_mode=peripheral` has to be stated explicitly because the
board has no OTG_ID line, and the USB-C connector is also the power input — so
**power the board through the GPIO header** when the gadget link is in use.
No DHCP server is installed, so give the host a static address in the same
subnet or rely on IPv4 link-local. See
[Raspberry Pi 5 notes](raspberry-pi-5.md#usb-gadget-mode).

**`uart.enabled` with `console = false`.** This is a *data* UART on GPIO
14/15 — `dtparam=uart0=on`, `/dev/ttyAMA0` — with no kernel console attached,
which is what talking to a peripheral needs. The widely repeated
`enable_uart=1` recipe means the dedicated debug connector on Pi 5, which is a
different port; the two are separate switches here for exactly that reason.

**Groups are not implied by buses.** Enabling I²C and SPI does not grant the
account access to them; `groups` is what does that. A group that does not
exist on the device is skipped with a warning rather than created.

**`authorized_keys_files`** reads the keys from a file next to the
specification, so the key material stays in one place instead of being pasted
into every specification.

**Your own files and commands.** `[[files]]` copies anything next to the
specification onto the root filesystem — `files/motd` as a single file,
`files/sysctl.d` as a directory copied recursively. `[[run]]` then runs
commands once everything else is in place. `apt-get update` needs a network,
which a bench board may not have on its very first boot, so this one tolerates
failure rather than marking the whole run failed.

For anything longer than one line, transfer a script and invoke it:

```toml
[[files]]
source = "files/setup.sh"
destination = "/usr/local/sbin/setup.sh"
mode = "0755"

[[run]]
command = "/usr/local/sbin/setup.sh"
```

## One specification, a fleet of boards

Keep the parts that are common in the file and override the rest per board:

```console
$ for n in 01 02 03; do
    rpi-provision apply fleet.toml --boot "$BOOT" --yes \
      --set system.hostname="dev-pi-$n" \
      --set "network.ethernet[0].address=192.168.1.$((10#$n + 50))/24" \
      --set-secret user.password_hash="env:PI${n}_HASH"
  done
```

`--set` values are visible in the process list, so anything confidential goes
through `--set-secret` instead. Each override changes the specification
digest, which is recorded on the card and in
`/var/lib/rpi-provision/status`, so a board in a drawer can be traced back to
the inputs that produced it.

## Reading the result before writing a card

`render` produces the whole payload in an ordinary directory, which is the
fastest way to see what a specification actually means:

```console
$ rpi-provision render examples/development.toml --out ./rendered
$ cat ./rendered/rpi-provision/steps/50-network.sh
```

Every generated script is POSIX `sh`, starts with `set -eu`, and can be run on
its own — see [first boot on the device](first-boot.md#running-a-step-by-hand).
