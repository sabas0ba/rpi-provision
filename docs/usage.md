---
title: Usage
nav_id: usage
---

# Usage

Six commands. Two of them write anything, and both tell you what they are
about to do first.

```
rpi-provision validate <SPEC>              Parse and validate a specification
rpi-provision render   <SPEC> --out DIR    Write the generated files to a directory
rpi-provision diff     <SPEC> --boot PATH  Show what apply would change
rpi-provision apply    <SPEC> --boot PATH  Write the provisioning payload to a card
rpi-provision revert   <SPEC> --boot PATH  Undo a previous apply
rpi-provision detect                       List mounted Raspberry Pi boot partitions
```

## Finding the card

`detect` enumerates mounted filesystems and reports the ones that look like a
Raspberry Pi boot partition, along with the model read from the firmware
blob. Each line is the mount point and the model, separated by a tab.

```console
$ rpi-provision detect
/media/user/bootfs	Raspberry Pi 5
```

On Windows the same command lists drive letters; see [Windows](windows.md).

## Checking before writing

`validate` parses the specification, resolves its secrets and runs every
check, then prints a summary — target, host name, account, connections and the
specification digest. It touches no card at all, which makes it the right
thing to run in CI over a repository of specifications.

```console
$ rpi-provision validate pi.toml
```

Unknown keys are an error rather than being ignored, so a misspelled key is
reported instead of silently doing nothing.

`render` writes everything that *would* go onto a card into an ordinary
directory, so you can read the generated `config.txt` block, the
NetworkManager keyfiles and the first-boot scripts before any card is
involved:

```console
$ rpi-provision render pi.toml --out ./rendered
```

`diff` compares the generated result against a real card and prints the
change set without writing:

```console
$ rpi-provision diff pi.toml --boot /media/$USER/bootfs
```

Diffs never print secret content.

## Applying

```console
$ rpi-provision apply pi.toml --boot /media/$USER/bootfs
```

`apply` prints the change set and asks for confirmation. With no terminal on
standard input it refuses to proceed unless `--yes` is given, so an unattended
script has to say so explicitly rather than being confirmed by accident.

Applying twice is a no-op:

- the `config.txt` managed block is replaced rather than appended, and stays
  at the end of the file — conditional filters such as `[all]` are sticky, so
  a block inserted in the middle would change the scope of everything after
  it;
- `cmdline.txt` is edited token by token, never by a blanket regular
  expression;
- every generated file is compared before it is written.

`revert` removes what `apply` added — the managed block, the command-line
tokens and the payload directory — leaving the rest of the card alone.

## Secrets

A specification is meant to be committed, so secret fields take a *source*
instead of a literal. A bare string in a secret field is rejected.

```toml
[user]
password_hash = { env = "RPI_PASSWORD_HASH" }   # from the environment
# password_hash = { file = "secrets/pw.hash" }  # from a file, relative to the spec
# password_hash = { value = "$6$..." }          # inline, only if you mean it
```

Exactly one of `env`, `file` or `value` must be present. A value read from a
file has its trailing newline removed.

The password hash is a crypt(3) hash, not a password — generate one with
`openssl passwd -6` or `mkpasswd --method=yescrypt`. A plain-text password is
rejected rather than hashed for you, because a specification that contains a
usable password is a specification you cannot commit.

## Overriding at run time

Anything can be overridden on the command line, which is how one
specification serves a fleet of boards:

```console
$ rpi-provision apply fleet.toml --boot /media/$USER/bootfs \
    --set system.hostname=dev-pi-07 \
    --set 'network.ethernet[0].address=192.168.1.57/24' \
    --set-secret user.password_hash=env:PI07_HASH
```

`--set PATH=VALUE` assigns into the parsed document before validation. The
value is parsed as a TOML scalar when it can be, and as a string otherwise, so
`--set ssh.port=2222` gives an integer while
`--set system.hostname=dev-pi-07` gives a string. Array elements are indexed
with `[n]`. Missing intermediate tables are created; a missing array element
is an error.

`--set-secret PATH=SOURCE` takes `env:NAME`, `file:PATH` or `value:LITERAL`.

> **Use `--set-secret` for anything confidential.** `--set` values are visible
> in the process list to every user on the machine.

Overrides change the specification digest, which is recorded in `config.txt`,
in `firstrun.sh` and in `/var/lib/rpi-provision/status` on the device — so a
board can always be traced back to the exact inputs that produced it.

## Options

| Option | Effect |
| --- | --- |
| `--boot PATH` | Mount point of the FAT boot partition |
| `--out DIR` | Output directory for `render` |
| `--set PATH=VALUE` | Override a value. Repeatable |
| `--set-secret PATH=SOURCE` | Override a secret's source: `env:NAME`, `file:PATH` or `value:LITERAL`. Repeatable |
| `-y`, `--yes` | Do not ask for confirmation before writing |
| `--allow-unverified-boot` | Skip the boot-partition sanity check |
| `--ignore-conflicts` | Proceed even though another first-boot mechanism is present |
| `-q`, `--quiet` | Only report errors |
| `-v`, `--verbose` | Show the full content of newly created files |

### The safety rails, and when to lower them

`apply` refuses to write to a directory that does not look like a Raspberry
Pi 5 boot partition. `--allow-unverified-boot` lowers that check — useful when
writing into a directory you will copy onto a card yourself, and a good way to
overwrite the wrong filesystem otherwise.

`apply` also refuses when the card already carries a competing first-boot
mechanism — `custom.toml`, `userconf.txt`, a foreign `firstrun.sh`. Two
mechanisms leave the order of operations undefined; the right fix is almost
always to delete the other one rather than to pass `--ignore-conflicts`.

## Automation

For an unattended run, provide the secrets through the environment and say
`--yes` explicitly:

```console
$ RPI_PASSWORD_HASH="$(cat secrets/pw.hash)" \
    rpi-provision apply fleet.toml --boot "$BOOT" --set system.hostname="$NAME" --yes
```

`validate` in CI over every specification in a repository catches typos, bad
CIDRs and missing regulatory domains long before a card is written.
