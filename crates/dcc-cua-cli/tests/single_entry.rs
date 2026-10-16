use rstest::rstest;
use std::process::Command;

fn captured(binary: &str, arguments: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary);
    command.args(arguments);
    command.output().expect("run owned pipe-only CLI")
}

#[rstest]
#[case(&["--version"], true)]
#[case(&["manifest"], true)]
#[case(&["__invalid_single_entry_contract_command"], false)]
fn single_entry_preserves_cli_output_and_exit_status(
    #[case] arguments: &[&str],
    #[case] success: bool,
) {
    let output = captured(env!("CARGO_BIN_EXE_dcc-cua"), arguments);
    assert_eq!(output.status.success(), success);
    assert_eq!(output.status.code(), Some(if success { 0 } else { 1 }));
    assert!(!output.stdout.is_empty(), "do not swallow result or error");
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
fn single_entry_retains_console_shell_contract_and_stack() {
    assert_eq!(
        pe_subsystem_and_stack(env!("CARGO_BIN_EXE_dcc-cua")),
        (3, 8 * 1024 * 1024)
    );
}

#[cfg(all(windows, target_env = "msvc"))]
#[rstest]
fn linked_manifest_contains_loader_console_policy() {
    // Read the actual linked RT_MANIFEST resource, not a source-string assertion.
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_dcc-cua")).unwrap();
    let u16_at = |i| u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()) as usize;
    let u32_at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
    let pe = u32_at(0x3c);
    let optional = pe + 24;
    let directories = optional + if u16_at(optional) == 0x20b { 112 } else { 96 };
    let section_table = optional + u16_at(pe + 20);
    let rva_offset = |rva: usize| {
        (0..u16_at(pe + 6))
            .find_map(|index| {
                let section = section_table + index * 40;
                let address = u32_at(section + 12);
                let size = u32_at(section + 8).max(u32_at(section + 16));
                (rva >= address && rva - address < size)
                    .then(|| u32_at(section + 20) + rva - address)
            })
            .expect("resource RVA belongs to a section")
    };
    let base = rva_offset(u32_at(directories + 16));
    let child = |directory: usize, id: Option<usize>| {
        let count = u16_at(directory + 12) + u16_at(directory + 14);
        (0..count)
            .find_map(|i| {
                let entry = directory + 16 + i * 8;
                if id.is_none_or(|id| u32_at(entry) == id) {
                    Some(u32_at(entry + 4))
                } else {
                    None
                }
            })
            .expect("manifest resource entry")
    };
    let kind = child(base, Some(24)); // RT_MANIFEST
    assert_ne!(kind & 0x80000000, 0);
    let identifier = child(base + (kind & 0x7fffffff), Some(1));
    assert_ne!(identifier & 0x80000000, 0);
    let data = child(base + (identifier & 0x7fffffff), None);
    assert_eq!(data & 0x80000000, 0);
    let descriptor = base + data;
    let offset = rva_offset(u32_at(descriptor));
    let manifest = std::str::from_utf8(&bytes[offset..offset + u32_at(descriptor + 4)]).unwrap();
    assert!(manifest.contains("http://schemas.microsoft.com/SMI/2024/WindowsSettings"));
    assert!(manifest.contains(">detached</consoleAllocationPolicy>"));
}
