//! Open a URL in the user's browser. The child is spawned detached; its output is ignored.

use std::process::{Command, Stdio};

pub fn open(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (program, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open a browser: {error}"))
}
