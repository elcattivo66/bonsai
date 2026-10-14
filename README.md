# bonsai

Grandfather-father-son (GFS) snapshot manager for [bcachefs](https://bcachefs.org/),
in the spirit of [snapper](https://github.com/openSUSE/snapper) and
[sanoid](https://github.com/jimsalterjrs/sanoid).

bonsai creates read-only bcachefs subvolume snapshots on a schedule and prunes
them according to a per-target retention policy. It is a Rust reimplementation
and evolution of a shell script.

## How it works

On each run, for every configured target:

1. **Create** the tiers due today (UTC):
   - `daily` — every day,
   - `weekly` — on Sundays,
   - `monthly` — on the 1st,
   - `yearly` — on January 1st.
2. **Prune** each tier down to the configured keep-count, keeping the newest.

Snapshots are named `<tier>-YYYYMMDD` and stored under
`<target>/<snapshot_subdir>/` (default `.snapshots`). They are read-only
subvolumes; browse with `ls`, restore with `cp -a` or `bcp`.

## Configuration

See [`config.example.toml`](config.example.toml):

```toml
snapshot_subdir = ".snapshots"

[[target]]
path = "/spool/garage"
retention = { daily = 7, weekly = 4, monthly = 6, yearly = 1 }
```

## Usage

```sh
# Preview what the daily run would do.
bonsai --config config.toml --dry-run snapshot

# Create snapshots and prune.
bonsai --config config.toml snapshot

# List existing snapshots.
bonsai --config config.toml list

# Restrict to a single target.
bonsai --config config.toml snapshot /spool/photos
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

## License

Licensed under the [GNU Affero General Public License v3.0](LICENSE)
(AGPL-3.0-only).
