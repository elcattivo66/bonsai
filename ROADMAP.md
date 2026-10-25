# Roadmap

Informal, prioritized roadmap. Versions are milestones, not promises.

## v0.1.0 — Released

- [x] Tag `v0.1.0`.
- Note: `cargo publish` is currently blocked by the self dev-dependency that
      enables the `test-util` feature for integration tests; publishing would
      require moving `MockBcachefs` into a separate workspace crate.

## v0.2.0 — Released

- [x] `hourly` tier — 10-digit names (`<tier>-YYYYMMDDHH`); the parser still
      accepts the 8-digit names.
- [x] `timezone` option (IANA name, default UTC) for local rollover.
- [x] `--json` output for `list` and `snapshot --dry-run`.
- [x] `delete` (interactive confirmation, locked) and `restore` (via `rsync`).
- [x] `snapshot --json` for a real run with per-target results.

## v0.3 — Robustness & integration

- [x] Sizes in `list` (`--human-readable`) and detection of unmanaged
      subvolumes via `bcachefs subvolume list --json -s` (conservative: the
      directory scan remains the planning basis, and `list` falls back to it
      when the query fails).
- [ ] Make the subvolume list the source of truth for planning/prune as well.
- [ ] Pre/post hooks per target (sanoid-style, e.g. database quiesce).
- [ ] Ship the NixOS module as `nixosModules.default` in this flake so
      package and module come from one source.

## v0.4 — Optional

- [ ] `diff` between snapshots.
- [ ] `bonsai status` with a state file / health reporting.
- [ ] node-exporter textfile metrics.
- [ ] TUI, if ever.

## Decisions

- **Writable snapshots (no `-r`):** bonsai deliberately does *not* create
  read-only snapshots; it uses the default (writable) bcachefs snapshots, as
  the original shell script did. Revisit read-only only if `bcachefs subvolume
  delete` is confirmed to delete read-only snapshots cleanly.
