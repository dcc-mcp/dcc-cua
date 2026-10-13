use rstest::rstest;
use std::process::Command;

fn captured(binary: &str, arguments: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary);
    command.args(arguments);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Keep the ordinary console CLI hidden during this pipe-only comparison.
        command.creation_flags(0x0800_0000);
    }
    command.output().expect("run owned pipe-only CLI")
}

#[rstest]
#[case(&["--version"], true)]
#[case(&["manifest"], true)]
#[case(&["__invalid_background_contract_command"], false)]
fn background_preserves_cli_output_and_exit_status(
    #[case] arguments: &[&str],
    #[case] success: bool,
) {
    let terminal = captured(env!("CARGO_BIN_EXE_dcc-cua"), arguments);
    let background = captured(env!("CARGO_BIN_EXE_dcc-cua-background"), arguments);
    assert_eq!(terminal.status.success(), success);
    assert_eq!(background.status.code(), terminal.status.code());
    assert_eq!(background.stdout, terminal.stdout);
    assert_eq!(background.stderr, terminal.stderr);
    assert!(
        !background.stdout.is_empty(),
        "do not swallow result or error"
    );
}

#[cfg(windows)]
fn pe_subsystem_and_stack(binary: &str) -> (u16, u64) {
    let bytes = std::fs::read(binary).expect("read linked executable");
    assert_eq!(&bytes[..2], b"MZ");
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    // Subsystem has the same offset in PE32 and PE32+ optional headers.
    let optional = pe + 4 + 20;
    let subsystem = u16::from_le_bytes(bytes[optional + 68..optional + 70].try_into().unwrap());
    let magic = u16::from_le_bytes(bytes[optional..optional + 2].try_into().unwrap());
    let stack = match magic {
        0x20b => u64::from_le_bytes(bytes[optional + 72..optional + 80].try_into().unwrap()),
        0x10b => u32::from_le_bytes(bytes[optional + 72..optional + 76].try_into().unwrap()) as u64,
        _ => panic!("unsupported PE optional header"),
    };
    (subsystem, stack)
}

#[cfg(windows)]
#[rstest]
fn background_never_requests_a_console_at_process_creation() {
    assert_eq!(
        pe_subsystem_and_stack(env!("CARGO_BIN_EXE_dcc-cua-background")),
        (2, 8 * 1024 * 1024)
    );
    assert_eq!(
        pe_subsystem_and_stack(env!("CARGO_BIN_EXE_dcc-cua")),
        (3, 8 * 1024 * 1024)
    );
}
