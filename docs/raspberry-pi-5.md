---
title: Raspberry Pi 5 notes
nav_id: pi5
---

# Raspberry Pi 5 specific behaviour

Several things changed between Raspberry Pi 4 and 5 in ways that make
configuration copied from older guides silently wrong. This file records what
the tool assumes and where that comes from.

## UART

On Raspberry Pi 5 the *primary* UART is the dedicated three-pin debug
connector in the corner of the board, not the 40-pin header.

| What you want | `config.txt` | Device node | `cmdline.txt` |
| --- | --- | --- | --- |
| A data UART on GPIO 14/15 | `dtparam=uart0=on` | `/dev/ttyAMA0` | remove `console=serial0,...` |
| A console on GPIO 14/15 | `dtparam=uart0=on` | `/dev/ttyAMA0` | `console=ttyAMA0,115200` |
| The debug connector | `enable_uart=1` | `/dev/ttyAMA10` | `console=serial0,115200` (the stock setting) |

`serial0` points at `/dev/ttyAMA10` on this board, so the widely repeated
"`enable_uart=1` plus `console=serial0`" recipe puts the console on the debug
connector rather than on the header. `raspi-config` has shipped this
inconsistency: it enables UART0 but leaves the kernel pointed at UART10.

`rpi-provision` therefore models the two as separate switches,
`hardware.uart.enabled` and `hardware.uart.debug_connector`, and writes
`console=ttyAMA0,<baudrate>` explicitly when a header console is requested.

Sources:

- <https://www.raspberrypi.com/documentation/computers/config_txt.html>
- <https://chris-besch.com/articles/raspberry_pi_5_uart/>

## USB gadget mode

Raspberry Pi 5 supports peripheral mode on the USB-C connector, but:

1. **`dr_mode` must be stated explicitly.** The board has no OTG_ID line, so
   role detection cannot happen automatically. The overlay is
   `dtoverlay=dwc2,dr_mode=peripheral`.
2. **The USB-C port is also the power input.** When a host is connected for
   the gadget link, the board draws power over that cable and may be current
   limited. Power the board through the GPIO header instead and leave USB-C
   for data.

The gadget itself is composed at run time through configfs by
`/usr/local/sbin/rpi-provision-gadget`, started by
`rpi-provision-gadget.service` before `network-pre.target`. `libcomposite` is
requested through `/etc/modules-load.d/` so the unit does not race the module
loader.

RNDIS additionally emits Microsoft OS descriptors (`qw_sign = MSFT100`,
compatible ID `RNDIS`, sub-compatible ID `5162001`), without which Windows
hosts will not bind the driver.

Source:

- *Using OTG mode on Raspberry Pi SBCs*, Raspberry Pi Ltd white paper
  RP-009276-WP,
  <https://pip-assets.raspberrypi.com/categories/685-app-notes-guides-whitepapers/documents/RP-009276-WP/Using-OTG-mode-on-Raspberry-Pi-SBCs>

## Networking

Raspberry Pi OS has used NetworkManager since Bookworm. Consequently:

- `wpa_supplicant.conf` placed on the boot partition is **ignored**. Wi-Fi is
  configured with a keyfile under `/etc/NetworkManager/system-connections/`.
- `dhcpcd.conf` is no longer the place for static addresses; the same keyfile
  carries `[ipv4] method=manual`.
- Keyfiles must be owned by root with mode 0600 or NetworkManager refuses to
  load them. The payload declares 0600 and the network step re-asserts it.

The Wi-Fi regulatory domain is set with
`raspi-config nonint do_wifi_country <CC>`, which also unblocks the radio.
Without it the interface stays soft-blocked.

## Boot partition layout

Since Bookworm the FAT partition is mounted at `/boot/firmware`, not `/boot`.
`provisioning.boot_mount` defaults accordingly; the `systemd.run=` hook and
the runner's own paths follow it.

## `config.txt` conditional filters

Filters such as `[all]`, `[pi5]` and `[cm5]` are *sticky*: every line after a
filter belongs to it until the next one. A block inserted in the middle of the
file would therefore change the scope of the lines that follow it.

`rpi-provision` keeps its managed block at the end of the file and starts it
with `[all]`. Re-applying moves the block back to the end if somebody has
appended lines after it.

## The first-boot hook

`rpi-imager` and `raspberrypi-sys-mods` both use the boot partition for
first-boot work, and they coexist with this tool as follows:

- `init=/usr/lib/raspberrypi-sys-mods/firstboot` runs first, resizes the root
  filesystem, and removes itself from `cmdline.txt`.
- `systemd.run=` then runs `firstrun.sh` at `kernel-command-line.target`.

`rpi-provision` does not use `custom.toml`, `rpi-preseed.toml`, `userconf.txt`
or the `ssh` marker file. Mixing two first-boot mechanisms makes the order of
operations undefined; everything here goes through one path. If the card
already carries a `custom.toml` or a foreign `firstrun.sh`, remove it before
applying.
