//! Small platform adapter for writing text to the system clipboard.

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Copy text using the native clipboard command available on this platform.
pub(super) fn copy(text: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let commands: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(target_os = "windows")]
    let commands: &[(&str, &[&str])] = &[("clip", &[])];
    #[cfg(all(unix, not(target_os = "macos")))]
    let commands: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    #[cfg(not(any(unix, target_os = "windows")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "clipboard is unsupported on this platform",
    ));

    #[cfg(any(unix, target_os = "windows"))]
    {
        let mut last_error = None;
        for (program, args) in commands {
            let child = Command::new(program)
                .args(*args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    last_error = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(text.as_bytes())?;
            let status = child.wait()?;
            if status.success() {
                return Ok(());
            }
            last_error = Some(io::Error::other(format!("{program} exited with {status}")));
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no clipboard command found")
        }))
    }
}
