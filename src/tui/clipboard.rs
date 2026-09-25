//! Small platform adapter for writing text to the system clipboard.

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Copy text using the native clipboard command available on this platform.
///
/// Native tools all need a display server. When none of them works (SSH, a
/// bare TTY, or a headless session) the text is handed to the terminal
/// emulator through an OSC 52 escape instead, which sets the clipboard on the
/// machine the user is actually sitting at.
pub(super) fn copy(text: &str) -> io::Result<()> {
    copy_with(text, copy_native, &mut io::stdout())
}

/// Try the native clipboard command, falling back to OSC 52 on the terminal.
///
/// The native and terminal writers are injected so the fallback can be tested
/// without a real display or terminal.
fn copy_with(
    text: &str,
    native: impl FnOnce(&str) -> io::Result<()>,
    terminal: &mut impl Write,
) -> io::Result<()> {
    if native(text).is_ok() {
        return Ok(());
    }
    write_osc52(terminal, text)?;
    terminal.flush()
}

/// Write `text` to the terminal clipboard using OSC 52.
fn write_osc52(terminal: &mut impl Write, text: &str) -> io::Result<()> {
    write!(terminal, "\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Standard base64 with padding, kept local to avoid a dependency just for
/// the OSC 52 payload.
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let bits = (first << 16) | (second << 8) | third;
        out.push(TABLE[((bits >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((bits >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((bits >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(bits & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Write `text` through the first native clipboard command that succeeds.
fn copy_native(text: &str) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("€".as_bytes()), "4oKs");
    }

    #[test]
    fn copy_with_falls_back_to_osc52_when_native_fails() {
        let mut terminal = Vec::new();
        copy_with(
            "hi",
            |_| Err(io::Error::other("xsel exited with exit status: 1")),
            &mut terminal,
        )
        .expect("OSC 52 fallback succeeds");
        assert_eq!(terminal, b"\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn copy_with_uses_native_when_it_succeeds() {
        let mut terminal = Vec::new();
        let called = std::cell::Cell::new(false);
        copy_with(
            "hi",
            |_| {
                called.set(true);
                Ok(())
            },
            &mut terminal,
        )
        .expect("native copy succeeds");
        assert!(called.get());
        assert!(terminal.is_empty(), "no terminal escape on the native path");
    }
}
