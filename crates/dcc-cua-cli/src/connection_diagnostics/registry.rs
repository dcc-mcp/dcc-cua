use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{
    CONNECTION_SCHEMA, ConnectionRecord, ConnectionState, ProcessStatus, process_identity,
};

pub(super) const MAX_REPORT_RECORDS: usize = 256;
const MAX_SCAN_ENTRIES: usize = 1024;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_TERMINAL_RECORDS: usize = 128;

pub(super) fn default_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(unix)]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    #[cfg(not(any(windows, unix)))]
    let base: Option<PathBuf> = None;
    base.filter(|path| path.is_absolute())
        .map(|path| path.join("dcc-cua").join("connections"))
}

fn reject_links(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0; // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    false
}

fn validate_directory(path: &Path) -> io::Result<()> {
    reject_links(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and does not dereference pointers.
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    Ok(())
}

fn prepare_directory(path: &Path) -> io::Result<()> {
    reject_links(path)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    validate_directory(path)
}

pub(super) fn publish(directory: &Path, record: &ConnectionRecord) -> io::Result<()> {
    prepare_directory(directory)?;
    let bytes = serde_json::to_vec(record).map_err(|_| io::ErrorKind::InvalidData)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4()));
    let destination = directory.join(format!("{}.json", record.connection_id));
    // An existing destination must be an ordinary private record; never follow
    // a substituted symlink or replace an unrelated object.
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        validate_record_metadata(&metadata)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        // Best effort diagnostics need atomic visibility, not a durable write
        // on every request. Sudden power loss can discard the newest snapshot.
        drop(file);
        replace_record(&temporary, &destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_record(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_record(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};
    let source = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let target = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are NUL-terminated local buffers retained for the call;
    // a single same-directory replacement avoids a missing-record interval.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn validate_record_metadata(metadata: &fs::Metadata) -> io::Result<()> {
    if is_link(metadata) || !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and does not dereference pointers.
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    Ok(())
}

fn valid_record_name(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    let identity = name
        .strip_prefix("mcp-connection-")?
        .strip_suffix(".json")?;
    let uuid = Uuid::parse_str(identity).ok()?;
    (uuid.get_version_num() == 4 && uuid.to_string() == identity)
        .then(|| format!("mcp-connection-{identity}"))
}

fn read_record(path: &Path, expected_id: &str) -> io::Result<ConnectionRecord> {
    validate_record_metadata(&fs::symlink_metadata(path)?)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    validate_record_metadata(&file.metadata()?)?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let record: ConnectionRecord =
        serde_json::from_slice(&bytes).map_err(|_| io::ErrorKind::InvalidData)?;
    if record.schema != CONNECTION_SCHEMA
        || record.connection_id != expected_id
        || record.transport != "stdio"
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(record)
}

pub(super) fn read_records(directory: &Path) -> (bool, bool, Vec<ConnectionRecord>) {
    if validate_directory(directory).is_err() {
        return (false, false, Vec::new());
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return (false, false, Vec::new());
    };
    let mut records = Vec::new();
    let mut truncated = false;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_SCAN_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let Some(connection_id) = valid_record_name(&entry.file_name()) else {
            continue;
        };
        if let Ok(record) = read_record(&entry.path(), &connection_id) {
            records.push(record);
        }
    }
    (true, truncated, records)
}

/// Writers retain a bounded recent history; the public query stays read-only.
/// Unknown process identity never qualifies a live-looking record for deletion.
pub(super) fn retain_recent_terminal_records(directory: &Path) {
    let (available, truncated, records) = read_records(directory);
    if !available || truncated {
        return;
    }
    let mut terminal = records
        .into_iter()
        .filter(|record| {
            record.state == ConnectionState::Closed
                || process_identity::status(&record.bridge) == ProcessStatus::Ended
        })
        .collect::<Vec<_>>();
    terminal.sort_by_key(|record| {
        std::cmp::Reverse(
            record
                .closed_at_unix_ms
                .unwrap_or(record.last_activity_at_unix_ms),
        )
    });
    for record in terminal.into_iter().skip(MAX_TERMINAL_RECORDS) {
        let path = directory.join(format!("{}.json", record.connection_id));
        // Recheck immutable identity and terminal state immediately before
        // removing only an old diagnostic record in the private directory.
        if let Ok(current) = read_record(&path, &record.connection_id) {
            if current.created_at_unix_ms == record.created_at_unix_ms
                && (current.state == ConnectionState::Closed
                    || process_identity::status(&current.bridge) == ProcessStatus::Ended)
            {
                let _ = fs::remove_file(path);
            }
        }
    }
}
