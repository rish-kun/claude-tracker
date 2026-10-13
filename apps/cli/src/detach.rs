//! Spawn `cctrack <args>` fully detached so hooks return immediately.

use std::process::{Command, Stdio};

pub fn spawn(args: &[String]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // survive the hook's process group being torn down
    }
    cmd.spawn().map(|_| ())
}
