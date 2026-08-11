---
title: First boot on the device
nav_id: first-boot
---

# What happens on the device

`apply` writes a payload to the boot partition and adds three tokens to
`cmdline.txt`:

```
systemd.run=/boot/firmware/rpi-provision/firstrun.sh
systemd.run_success_action=reboot
systemd.unit=kernel-command-line.target
```

## Layout on the card

```
/boot/firmware/
    config.txt                  managed block appended at the end
    cmdline.txt                 the three tokens above
    rpi-provision/
        firstrun.sh             the runner
        manifest.tsv            mode, source, destination
        authorized_keys         installed with user ownership by step 40
        secrets/
            password.hash       never installed; read by step 20
        payload/                mirrors the target paths
            etc/...
            usr/local/sbin/...
        steps/
            10-hostname.sh
            20-user.sh
            30-payload.sh
            40-ssh.sh
            50-network.sh       only when networking is configured
            60-usb-gadget.sh    only when the gadget is enabled
            70-locale.sh        only when localisation is configured
```

## Sequence

1. `raspberrypi-sys-mods`' own `init=` hook runs first, resizes the root
   filesystem, removes itself from `cmdline.txt` and reboots.
2. systemd boots into `kernel-command-line.target` and runs `firstrun.sh`.
3. The runner copies `/boot/firmware/rpi-provision` into `/run/rpi-provision`
   (a tmpfs, mode 0700) and re-executes itself from there. From this point
   the FAT copy can be deleted while the script is still running, and the
   secrets are in RAM rather than on a world-readable filesystem.
4. `steps/*.sh` run in numeric order. Each runs in its own `/bin/sh` with
   `set -eu` and `RPI_PROVISION_BASE` pointing at the staging directory.
   Output goes to `/var/log/rpi-provision.log`. The first failing step stops
   the sequence.
5. The runner removes its own three tokens from `cmdline.txt`.
6. Unless `provisioning.wipe_payload = false`, `/boot/firmware/rpi-provision`
   is deleted.
7. `/var/lib/rpi-provision/status` is written with `status=ok` or
   `status=failed`, the specification digest and the generator version.
8. The runner exits 0, so `systemd.run_success_action=reboot` takes the board
   into a normal boot.

The runner always exits 0, including after a failed step. A failure that left
the hooks in place would produce a boot loop, which is far harder to recover
from than a machine that boots with an incomplete configuration and a log
explaining why.

## The steps

| Step | What it does |
| --- | --- |
| `10-hostname` | Writes `/etc/hostname`, calls `hostname`, keeps the `127.0.1.1` line in `/etc/hosts` consistent |
| `20-user` | Creates the account with `useradd --create-home --user-group` if absent, sets the shell, applies the password hash with `chpasswd --encrypted` (or locks the account), adds supplementary groups that exist |
| `30-payload` | Reads `manifest.tsv` and runs `install -D -m <mode>` for each entry, so the files land owned by root because the runner is root. Anything landing in `/etc/sudoers.d/` is checked with `visudo -c` |
| `40-ssh` | Installs `authorized_keys` into the account's `~/.ssh` with the right ownership, validates `sshd` configuration when host keys already exist, enables or disables `ssh.service` |
| `50-network` | Re-asserts root ownership and mode 0600 on the NetworkManager keyfiles, sets the Wi-Fi regulatory domain, unblocks the radio, reloads NetworkManager if it is already running |
| `60-usb-gadget` | `systemctl daemon-reload` and enables `rpi-provision-gadget.service` |
| `70-locale` | `raspi-config nonint do_change_timezone` / `do_change_locale` / `do_configure_keyboard` |

## Debugging

Attach a serial console (`hardware.uart.debug_connector = true`) or read the
card afterwards.

- `/var/log/rpi-provision.log` — the transcript, including each step's output.
- `/var/lib/rpi-provision/status` — `status=ok` or `status=failed`, plus the
  specification digest so you can tell which specification produced this card.
- If the run never started, check that `cmdline.txt` still carries the
  `systemd.run=` token: something else may have rewritten it.

To iterate, re-run `rpi-provision apply` on the card. The managed block and
the command line hooks are replaced rather than duplicated, so a card can be
re-provisioned any number of times.

Set `provisioning.wipe_payload = false` while debugging so that the scripts
remain on the card for inspection — but remember that this leaves the Wi-Fi
pre-shared key and the password hash readable by anyone who has the card.

## Running a step by hand

Every step is a standalone POSIX shell script that takes its input from
`RPI_PROVISION_BASE`:

```console
# RPI_PROVISION_BASE=/boot/firmware/rpi-provision \
    sh /boot/firmware/rpi-provision/steps/50-network.sh
```

That is also how the test suite exercises them.
