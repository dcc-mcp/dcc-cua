//! Failure-only output from the owned Electron fixture, without environment dumps.
use rstest::rstest;
use std::io::{Read, Seek, SeekFrom};

const MAX_TAIL_BYTES: u64 = 16 * 1024;

fn tail(file: &mut std::fs::File) -> std::io::Result<String> {
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(MAX_TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    file.take(MAX_TAIL_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(feature = "gui-e2e")]
pub(super) struct FixtureOutput(tempfile::NamedTempFile);

#[cfg(feature = "gui-e2e")]
impl FixtureOutput {
    pub(super) fn new() -> Self {
        Self(tempfile::NamedTempFile::new().expect("create owned fixture output"))
    }

    pub(super) fn stdout(&self) -> std::process::Stdio {
        self.0
            .as_file()
            .try_clone()
            .expect("clone fixture output")
            .into()
    }
}

#[cfg(feature = "gui-e2e")]
impl Drop for FixtureOutput {
    fn drop(&mut self) {
        if std::thread::panicking() {
            match tail(self.0.as_file_mut()) {
                Ok(output) => eprintln!("owned Electron fixture stdout tail (max16KiB): {output}"),
                Err(error) => eprintln!("owned Electron fixture stdout unreadable: {error}"),
            }
        }
    }
}

#[rstest]
#[case(0)]
#[case(12)]
#[case(20_000)]
fn owned_fixture_output_tail_is_bounded_and_includes_the_latest_bytes(#[case] prefix: usize) {
    use std::io::Write;
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&vec![b'x'; prefix]).unwrap();
    file.write_all(b"renderer-end").unwrap();
    let result = tail(&mut file).unwrap();
    assert!(result.ends_with("renderer-end"));
    assert_eq!(result.len(), (prefix + 12).min(MAX_TAIL_BYTES as usize));
}

#[rstest]
fn owned_fixture_output_tail_retains_unreadable_bytes_without_panicking() {
    use std::io::Write;
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&[0xff, b'a']).unwrap();
    assert_eq!(tail(&mut file).unwrap(), "\u{fffd}a");
}
