//! File-only immutable epoch records and the stable per-target recovery gate.
use super::*;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

fn failure(reason: PreparationFailure) -> PreparationError {
    PreparationError::new(reason)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn ordinary_directory_chain(path: &Path) -> bool {
    path.ancestors()
        .all(|ancestor| match fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata.is_dir() && !is_reparse(&metadata),
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        })
}

pub(super) fn ordinary_journal_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file() && !is_reparse(&metadata))
}

#[derive(Clone)]
pub(super) struct Journal {
    pub directory: PathBuf,
}

impl Journal {
    /// Call while holding the exact-target OS gate. No native APIs run here.
    pub fn prepare(spec: &CapturePreparationSpec) -> Result<Self, PreparationError> {
        spec.validate()?;
        if !ordinary_directory_chain(&spec.journal_directory) {
            return Err(failure(PreparationFailure::JournalFailed));
        }
        fs::create_dir_all(&spec.journal_directory)
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
        if !ordinary_directory_chain(&spec.journal_directory) {
            return Err(failure(PreparationFailure::JournalFailed));
        }
        let root = fs::canonicalize(&spec.journal_directory)
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
        let index = root.join(format!(
            "target-{}-{}-{}.json",
            spec.target.process_id,
            spec.target.native_instance.process_creation_time_100ns,
            spec.target.window_handle
        ));
        if fs::symlink_metadata(&index).is_ok() {
            if !ordinary_journal_file(&index) {
                return Err(failure(PreparationFailure::JournalFailed));
            }
            let old: PathBuf = serde_json::from_slice(
                &fs::read(&index).map_err(|_| failure(PreparationFailure::JournalFailed))?,
            )
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
            // Read no prior record until its canonical epoch directory is proved
            // to be an ordinary immediate child of this trusted journal root.
            if !ordinary_directory_chain(&old) {
                return Err(failure(PreparationFailure::GateBusy));
            }
            let old = fs::canonicalize(old).map_err(|_| failure(PreparationFailure::GateBusy))?;
            if old.parent() != Some(root.as_path()) {
                return Err(failure(PreparationFailure::GateBusy));
            }
            let binding_path = old.join("binding.json");
            let settled_path = old.join("settled.json");
            if !ordinary_journal_file(&binding_path) || !ordinary_journal_file(&settled_path) {
                return Err(failure(PreparationFailure::GateBusy));
            }
            let binding: CapturePreparationSpec = serde_json::from_slice(
                &fs::read(binding_path).map_err(|_| failure(PreparationFailure::GateBusy))?,
            )
            .map_err(|_| failure(PreparationFailure::GateBusy))?;
            let settled: CapturePreparationStatus = serde_json::from_slice(
                &fs::read(&settled_path).map_err(|_| failure(PreparationFailure::GateBusy))?,
            )
            .map_err(|_| failure(PreparationFailure::GateBusy))?;
            let expected_epoch = uuid::Uuid::from_bytes(binding.preparation_id).to_string();
            if binding.validate().is_err()
                || old.file_name() != Some(std::ffi::OsStr::new(&expected_epoch))
                || fs::canonicalize(&binding.journal_directory).ok().as_ref() != Some(&root)
                || binding.target != spec.target
                || settled.preparation_id != binding.preparation_id
                || settled.journal_path != old
                || !settled.cleanup_verified
                || !settled.capture_revoked
                || settled.pending_sequence.is_some()
                || !matches!(
                    settled.phase,
                    PreparationPhase::Restored | PreparationPhase::Refused
                )
            {
                return Err(failure(PreparationFailure::GateBusy));
            }
            OpenOptions::new()
                .write(true)
                .open(&settled_path)
                .and_then(|file| file.sync_all())
                .map_err(|_| failure(PreparationFailure::JournalFailed))?;
            fs::remove_file(&index).map_err(|_| failure(PreparationFailure::JournalFailed))?;
        }
        let directory = root.join(uuid::Uuid::from_bytes(spec.preparation_id).to_string());
        fs::create_dir(&directory).map_err(|_| failure(PreparationFailure::JournalFailed))?;
        let journal = Self { directory };
        journal.record("binding", spec)?;
        let bytes = serde_json::to_vec(&journal.directory)
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&index)
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
        if fs::read(index).map_err(|_| failure(PreparationFailure::JournalFailed))? != bytes {
            return Err(failure(PreparationFailure::JournalFailed));
        }
        Ok(journal)
    }

    pub fn record<T: Serialize>(&self, name: &str, value: &T) -> Result<(), PreparationError> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(failure(PreparationFailure::JournalFailed));
        }
        let bytes =
            serde_json::to_vec(value).map_err(|_| failure(PreparationFailure::JournalFailed))?;
        let path = self.directory.join(format!("{name}.json"));
        let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !ordinary_journal_file(&path)
                    || fs::read(&path).ok().as_deref() != Some(bytes.as_slice())
                {
                    return Err(failure(PreparationFailure::JournalFailed));
                }
                return OpenOptions::new()
                    .write(true)
                    .open(path)
                    .and_then(|file| file.sync_all())
                    .map_err(|_| failure(PreparationFailure::JournalFailed));
            }
            Err(_) => return Err(failure(PreparationFailure::JournalFailed)),
        };
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| failure(PreparationFailure::JournalFailed))?;
        if fs::read(&path).map_err(|_| failure(PreparationFailure::JournalFailed))? != bytes {
            return Err(failure(PreparationFailure::JournalFailed));
        }
        Ok(())
    }
}
