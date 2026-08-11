# Using rpi-provision from Windows

The tool writes only to the FAT32 boot partition, which Windows mounts as an
ordinary drive letter. No WSL, no Docker, no ext4 driver, no elevation.

```powershell
PS> rpi-provision detect
D:\    Raspberry Pi 5

PS> $env:RPI_PASSWORD_HASH = "<crypt(3) hash>"
PS> rpi-provision diff  pi.toml --boot D:\
PS> rpi-provision apply pi.toml --boot D:\
```

## Producing a password hash on Windows

`openssl passwd -6` is the usual recipe but is not present by default. Any of
these work:

- Git for Windows ships OpenSSL: `"C:\Program Files\Git\usr\bin\openssl.exe" passwd -6`
- WSL, if installed: `wsl openssl passwd -6`
- On another machine, then paste into the environment variable.

Do not put the hash on the command line with `--set`: it would be visible in
the process list. Use the environment variable, a file, or `--set-secret`.

```powershell
PS> rpi-provision apply pi.toml --boot D:\ --set-secret user.password_hash=file:secrets\pw.hash
```

## Line endings

Everything the tool generates uses LF, on every platform. This matters:

- `cmdline.txt` must be a single line; the firmware does not tolerate CRLF.
- `firstrun.sh` and the step scripts are run by `/bin/sh` (dash) on the
  device, which fails on `\r` in a shebang or in a `case` pattern.

If you edit a generated file by hand on Windows, make sure your editor keeps
LF. `rpi-provision apply` will rewrite it correctly anyway.

## Path form

`--boot` accepts either separator; `D:\`, `D:/` and `D:` all work. Paths
*inside* the generated payload always use `/`, because they are consumed on
Linux.

## Ejecting

Windows caches writes to removable media. Use "Safely Remove Hardware" (or
`Dismount-Volume`) before pulling the card, otherwise the payload may be
incomplete. `rpi-provision` calls no flush of its own beyond closing the
files.

## What still needs Linux

Writing the OS image to the card is out of scope for this tool. Use
Raspberry Pi Imager, which is available for Windows. If you want to customise
a `.img` file before writing it, that does need a loop mount and therefore
WSL2 or a Linux host — but the whole point of the boot-partition-only design
is that you do not have to.
