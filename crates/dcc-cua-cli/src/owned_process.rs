use std::ffi::OsStr;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedConsoleChildRole {
    NativeMessagingRegistry,
}

#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW_FLAG: u32 = 0x0800_0000;

pub(crate) fn command(_role: OwnedConsoleChildRole, program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW_FLAG);
    }
    command
}
