# Roadmap

Informal, prioritized roadmap. Versions are milestones, not promises.

## v0.1.1 — Hardening (now)

- [ ] Verify on a real bcachefs host that `bcachefs subvolume delete` works on
      read-only snapshots (created via `subvolume snapshot -r`); revert `-r`
      if pruning breaks because of it.
- [ ] Tag `v0.1.0`.
- Note: `cargo publish` is currently blocked by the self dev-dependency that
      enables the `test-util` feature for integration tests; publishing would
      require moving `MockBcachefs` into a separate workspace crate.

## v0.2 — snapper/sanoid parity

- [ ] `hourly` tier — decide the name format (`<tier>-YYYYMMDDHH`) before many
      day-format snapshots accumulate; the parser must keep accepting the
      existing 8-digit names.
- [ ] `timezone` option for local rollover instead of UTC.
- [ ] `--json` output for `list` and `snapshot --dry-run`.
- [ ] `delete` and `restore` commands (`cp -a --reflink` for copy-on-write).

## v0.3 — Robustness & integration

- [ ] Inventory via `bcachefs subvolume list --json` instead of directory
      names: source of truth, catches manually created subvolumes, and
      provides sizes for `list`.
- [ ] Pre/post hooks per target (sanoid-style, e.g. database quiesce).
- [ ] Ship the NixOS module as `nixosModules.default` in this flake so
      package and module come from one source.

## v0.4 — Optional

- [ ] `diff` between snapshots.
- [ ] `bonsai status` with a state file / health reporting.
- [ ] node-exporter textfile metrics.
- [ ] TUI, if ever.
