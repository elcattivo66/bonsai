use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Copies a restored file or subtree into place.
///
/// Implemented for real with `rsync` and mocked in tests. `rsync` is used for
/// both files and directories; note that it does not reflink, so a restore
/// copies real data (no copy-on-write space sharing).
pub trait Restore {
    /// Copy `source` (a file or directory) to `dest`.
    fn restore(&self, source: &Path, dest: &Path) -> Result<()>;
}

/// The real implementation, shelling out to `rsync`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RsyncRestore;

impl Restore for RsyncRestore {
    fn restore(&self, source: &Path, dest: &Path) -> Result<()> {
        let mut command = Command::new("rsync");
        // -aHAX: archive + hardlinks + ACLs + xattrs. Never --delete: the
        // destination is not emptied. Nested snapshots are excluded.
        command.args(["-aHAX", "--exclude=.snapshots"]);
        if source.is_dir() {
            // Copy the *contents* of the directory into the destination.
            command
                .arg(trailing_slash(source))
                .arg(trailing_slash(dest));
        } else {
            command.arg(source).arg(dest);
        }

        let described = describe(&command);
        let output = command
            .output()
            .with_context(|| format!("running {described}"))?;
        if !output.status.success() {
            bail!(
                "{described} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

fn trailing_slash(path: &Path) -> String {
    let mut path = path.to_string_lossy().into_owned();
    if !path.ends_with('/') {
        path.push('/');
    }
    path
}

/// Render a command as a human-readable string for logs and errors.
fn describe(command: &Command) -> String {
    let program = command.get_program().to_string_lossy().into_owned();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if args.is_empty() {
        program
    } else {
        format!("{program} {args}")
    }
}
