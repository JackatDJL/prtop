//! Copying text without a hard dependency: OSC52 first (works from the TUI itself), then
//! the usual platform helpers. Failure is reported to the user, never fatal.

pub fn copy(text: &str) -> Result<(), String> {
    osc52(text);
    for command in fallback_commands() {
        let (program, args) = command;
        if let Ok(mut child) = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return Ok(());
        }
    }
    // OSC52 may still have reached a supporting terminal even without a helper binary.
    Err("clipboard helper unavailable; OSC52 sent".into())
}

fn osc52(text: &str) {
    use std::io::Write;
    let encoded = base64(text);
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

fn fallback_commands() -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", vec![])];
    }
    if cfg!(target_os = "windows") {
        return vec![("clip", vec![])];
    }
    vec![
        ("wl-copy", vec![]),
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
    ]
}

fn base64(text: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_encoding() {
        assert_eq!(base64("hello"), "aGVsbG8=");
        assert_eq!(base64("hello world"), "aGVsbG8gd29ybGQ=");
        assert_eq!(base64("a"), "YQ==");
    }
}
