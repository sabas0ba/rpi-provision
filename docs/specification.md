---
title: Specification
nav_id: specification
---

# Specification reference

A specification is a TOML document. Unknown keys are an error, so a typo is
reported rather than silently ignored.

Validate one with:

```console
$ rpi-provision validate pi.toml
```

## Secret values

Fields marked *secret* below take a source, not a literal:

```toml
password_hash = { env  = "RPI_PASSWORD_HASH" }
password_hash = { file = "secrets/pw.hash" }     # relative to the spec file
password_hash = { value = "$6$..." }             # discouraged
```

Exactly one of `env`, `file` or `value` must be present. A bare string is
rejected. Values read from a file have their trailing newline removed.

Override the source at run time with
`--set-secret <path>=env:NAME|file:PATH|value:LITERAL`.

## `[meta]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `schema_version` | integer | `1` | Must equal the version this build understands |
| `target` | string | `"pi5"` | Only `pi5` is supported |
| `description` | string | — | Free text, ignored by the tool |

## `[system]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `hostname` | string | *required* | RFC 1123 label: letters, digits and hyphens, 1–63 characters, no leading or trailing hyphen |
| `timezone` | string | — | e.g. `Asia/Tokyo`; applied with `raspi-config nonint do_change_timezone` |
| `locale` | string | — | e.g. `en_US.UTF-8` |
| `keymap` | string | — | e.g. `jp` |

## `[user]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `name` | string | *required* | Lowercase letters, digits, `_` and `-`; 1–32 characters |
| `password_hash` | *secret* | — | A crypt(3) hash. Generate with `openssl passwd -6` or `mkpasswd --method=yescrypt`. A plain-text password is rejected |
| `authorized_keys` | array of strings | `[]` | Full `ssh-ed25519 AAAA... comment` lines |
| `authorized_keys_files` | array of strings | `[]` | Paths relative to the spec file; comments and blank lines are skipped |
| `groups` | array of strings | `[]` | Supplementary groups. A group that does not exist on the device is skipped with a warning rather than created |
| `shell` | string | `/bin/bash` | Absolute path |
| `sudo` | string | `nopasswd` | `nopasswd`, `password` or `none`. The first two add the account to `sudo`; `nopasswd` also installs `/etc/sudoers.d/010-rpi-provision-<user>` |

At least one of `password_hash` and `authorized_keys` must be present.

## `[ssh]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `enabled` | boolean | `true` | `false` disables and masks `ssh.service` |
| `port` | integer | `22` | 1–65535 |
| `password_authentication` | boolean | `false` | When `false`, at least one authorized key is required |
| `permit_root_login` | string | `no` | `no`, `yes`, `prohibit-password`, `forced-commands-only` |
| `extra_config` | array of strings | `[]` | Appended verbatim to `/etc/ssh/sshd_config.d/10-rpi-provision.conf` |

## `[network]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `wifi_country` | string | — | Two uppercase letters, ISO 3166-1 alpha-2. Required when any `[[network.wifi]]` is present: the radio stays blocked without a regulatory domain |

### `[[network.ethernet]]` and `[[network.wifi]]`

Shared keys:

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `id` | string | *required* | Becomes the keyfile name; unique across all connections |
| `interface` | string | `eth0` / `wlan0` | Linux interface name |
| `method` | string | `auto` | `auto` (DHCP), `manual` or `disabled` |
| `address` | string | — | CIDR, e.g. `192.168.1.50/24`. Required when `method = "manual"`, rejected otherwise |
| `gateway` | string | — | Only with `method = "manual"`. A gateway outside the subnet produces a warning |
| `dns` | array of strings | `[]` | IPv4 addresses |
| `ignore_auto_dns` | boolean | derived | Defaults to true when `dns` is set and `method = "auto"` |
| `ipv6` | string | `auto` | `auto` or `disabled` |
| `autoconnect` | boolean | `true` | |
| `autoconnect_priority` | integer | `0` | −999 to 999; higher wins |

Ethernet only:

| Key | Type | Notes |
| --- | --- | --- |
| `mac` | string | Sets `cloned-mac-address` |

Wi-Fi only:

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `ssid` | string | *required* | 1–32 bytes, no control characters |
| `security` | string | `wpa-psk` | `wpa-psk` (WPA2), `sae` (WPA3) or `open` |
| `psk` | *secret* | — | 8–63 character passphrase or 64 hex digits. Required unless `security = "open"` |
| `hidden` | boolean | `false` | |

A statically addressed wired connection is written with `may-fail=false`, so
`network-online.target` waits for it. Everything else uses `may-fail=true`.

### `[network.usb_gadget]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `enabled` | boolean | `false` | |
| `function` | string | `ecm` | `ecm` (Linux, macOS), `ncm` (higher throughput, also recent Windows) or `rndis` (older Windows; Microsoft OS descriptors are emitted) |
| `interface` | string | `usb0` | |
| `address` | string | `10.55.0.1/24` | The address the Pi takes on the link |
| `peer_address` | string | — | The host's address; must be inside `address` and differ from it. Informational, used for documentation and validation |
| `device_mac` | string | derived | Derived from the host name and interface, locally administered |
| `host_mac` | string | derived | As above |
| `vendor_id` | integer | `0x1d6b` | |
| `product_id` | integer | `0x0104` | |
| `manufacturer` | string | `Raspberry Pi` | |
| `product` | string | `rpi-provision USB gadget` | |
| `serial` | string | the host name | |

The link is configured with `never-default=true` and `may-fail=true`: it must
never become the default route, and an unplugged cable must not delay boot.

No DHCP server is installed on the device. Give the host a static address in
the same subnet, or rely on IPv4 link-local.

## `[hardware]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `pcie_gen` | integer | — | 1–3; emits `dtparam=pciex1_gen=` |
| `overlays` | array of strings | `[]` | Each becomes `dtoverlay=<value>` |
| `dtparams` | array of strings | `[]` | Each becomes `dtparam=<value>` |
| `config_extra` | array of strings | `[]` | Emitted verbatim into the managed block |
| `cmdline_append` | array of strings | `[]` | Whitespace-free tokens appended to `cmdline.txt` |
| `cmdline_remove` | array of strings | `[]` | Exact tokens removed from `cmdline.txt` |

### `[hardware.uart]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `enabled` | boolean | `false` | `dtparam=uart0=on`: UART0 on GPIO 14/15, `/dev/ttyAMA0` |
| `console` | boolean | `false` | Attach a kernel console to `/dev/ttyAMA0`. Requires `enabled` |
| `baudrate` | integer | `115200` | Used for the console |
| `debug_connector` | boolean | `false` | `enable_uart=1`: the dedicated three-pin debug connector, `/dev/ttyAMA10` |

With `console = false` and `debug_connector = false`, the stock
`console=serial0,115200` is removed so that GPIO 14/15 is a plain data port.

### `[hardware.i2c]`, `[hardware.spi]`, `[hardware.one_wire]`

| Section | Key | Type | Default |
| --- | --- | --- | --- |
| `i2c` | `enabled` | boolean | `false` |
| `i2c` | `baudrate` | integer | `100000` (10 000–1 000 000) |
| `spi` | `enabled` | boolean | `false` |
| `one_wire` | `enabled` | boolean | `false` |
| `one_wire` | `gpio` | integer | `4` (0–27) |

Enabling a bus does not grant the account access to it; add `i2c`, `spi` or
`gpio` to `user.groups` for that.

## `[provisioning]`

| Key | Type | Default | Notes |
| --- | --- | --- | --- |
| `boot_mount` | string | `/boot/firmware` | Where the boot partition is mounted **on the device** |
| `runner_dir` | string | `rpi-provision` | Directory below the boot partition holding the payload |
| `wipe_payload` | boolean | `true` | Delete the payload after a successful first boot. Leaving it in place keeps the Wi-Fi key and password hash readable by anyone with the card |
| `reboot_after` | boolean | `true` | |
| `log_path` | string | `/var/log/rpi-provision.log` | |

## Command line overrides

`--set <path>=<value>` assigns into the parsed document before validation.
The value is parsed as a TOML scalar when possible and as a string otherwise,
so `--set ssh.port=2222` gives an integer and `--set system.hostname=dev-pi-07`
gives a string. Array elements are indexed:

```console
--set 'network.ethernet[0].address=192.168.1.57/24'
```

Missing intermediate tables are created; a missing array element is an error.

Overrides change the specification digest, which is recorded in `config.txt`,
in `firstrun.sh` and in `/var/lib/rpi-provision/status` on the device.
