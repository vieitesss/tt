//! Launching the user's editor.

use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Open `path` in `$EDITOR` (whitespace-separated arguments allowed), falling
/// back to `vi` when `$EDITOR` is unset or empty.
///
/// # Errors
///
/// Returns an error when the editor cannot be launched or exits non-zero.
pub(crate) fn open(path: &Path) -> Result<()> {
    let mut command = editor();
    command.arg(path);
    let status = command.status().with_context(|| {
        format!(
            "launching `{}` for {}",
            command.get_program().to_string_lossy(),
            path.display()
        )
    })?;
    if !status.success() {
        bail!("editor exited with {status}");
    }
    Ok(())
}

/// The editor command from `$EDITOR`, with `vi` as the fallback.
pub(crate) fn editor() -> Command {
    editor_from(env::var("EDITOR").ok().as_deref())
}

/// [`editor`] against an explicit `$EDITOR` value.
pub(crate) fn editor_from(editor_var: Option<&str>) -> Command {
    let editor = editor_var.unwrap_or_default();
    let mut parts = editor.split_whitespace();
    let program = parts.next().filter(|part| !part.is_empty()).unwrap_or("vi");
    let mut command = Command::new(program);
    command.args(parts);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_splits_whitespace_and_falls_back_to_vi() {
        let command = editor_from(Some("  nano -w  "));
        assert_eq!(command.get_program().to_string_lossy(), "nano");
        let args: Vec<String> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-w"]);

        assert_eq!(
            editor_from(Some("   ")).get_program().to_string_lossy(),
            "vi"
        );
        assert_eq!(editor_from(None).get_program().to_string_lossy(), "vi");
    }
}
