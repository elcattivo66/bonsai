# bonsai

Grandfather-father-son (GFS) snapshot manager for [bcachefs](https://bcachefs.org/),
in the spirit of [snapper](https://github.com/openSUSE/snapper) and
[sanoid](https://github.com/jimsalterjrs/sanoid).

bonsai creates bcachefs subvolume snapshots on a schedule and prunes
them according to a per-target retention policy. It is a Rust reimplementation
and evolution of a shell script.

## How it works

On each run, for every configured target:

1. **Create** the tiers due today (UTC):
   - `hourly` — every run (disabled by default; needs an hourly timer),
   - `daily` — every day,
   - `weekly` — on Sundays,
   - `monthly` — on the 1st,
   - `yearly` — on January 1st.
2. **Prune** each tier down to the configured keep-count, keeping the newest.

A retention of `0` disables that tier: no new snapshots are created for it,
but any existing snapshots of that tier are still pruned.

`hourly` is off by default. If you enable it, the systemd timer must actually
fire at least hourly (the schedule is not derived from the retention), otherwise
you only get one hourly snapshot per timer run.

Snapshots are named `<tier>-YYYYMMDD` and stored under
`<target>/<snapshot_subdir>/` (default `.snapshots`). They are regular
bcachefs snapshots; browse with `ls`, restore with `cp -a` (or
`cp --reflink=auto` for copy-on-write clones within the same filesystem).

Failures are isolated per target: a target whose snapshots cannot be read,
created or deleted does not stop the remaining targets — but the run exits
non-zero if anything failed. Overlapping runs are prevented by a lock at
`/var/lock/bonsai.lock`; a second run started while one is in progress
exits successfully without doing anything.

## Configuration

See [`config.example.toml`](config.example.toml):

```toml
snapshot_subdir = ".snapshots"
timezone = "Europe/Berlin"

[[target]]
path = "/spool/garage"
retention = { hourly = 24, daily = 7, weekly = 4, monthly = 6, yearly = 1 }
```

`timezone` (IANA name, default `UTC`) controls day/hour boundaries and how
snapshots are named. Changing it does not rename existing snapshots — they keep
the names they were created with, so a boundary day may appear twice or be
skipped once across the change.

## Usage

```sh
# Preview what the daily run would do.
bonsai --config config.toml snapshot --dry-run

# Create snapshots and prune.
bonsai --config config.toml snapshot

# List existing snapshots.
bonsai --config config.toml list

# Restrict to a single target.
bonsai --config config.toml snapshot /spool/photos

# Machine-readable output (JSON on stdout, logs stay on stderr).
bonsai --config config.toml list --json
bonsai --config config.toml snapshot --dry-run --json
```

## Build

```sh
cargo build --release
```

With Nix:

```sh
nix build
nix develop
```

## Scheduling

Run `bonsai snapshot` from a systemd timer (e.g. daily). On NixOS this is
provided by the module in the author's `nix-config` flake.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Roadmap

See [ROADMAP.md](ROADMAP.md).

## License

Licensed under the [GNU Affero General Public License v3.0](LICENSE)
(AGPL-3.0-only).
