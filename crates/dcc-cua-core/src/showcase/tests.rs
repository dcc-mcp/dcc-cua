use rstest::rstest;

use std::io::BufReader;
use std::sync::Arc;

use super::*;

#[rstest]
fn showcase_dimensions_are_even_and_bounded() {
    assert_eq!(fit_dimensions(3840, 2400), (1440, 900));
    assert_eq!(fit_dimensions(1513, 949), (1434, 900));
    assert_eq!(
        fit_dimensions_with_bounds(3120, 2080, 1568, 1568),
        (1568, 1044)
    );
}

#[rstest]
fn annex_b_start_codes_are_removed() {
    assert_eq!(strip_start_code(&[0, 0, 0, 1, 0x67]), Some(&[0x67][..]));
    assert_eq!(strip_start_code(&[0, 0, 1, 0x68]), Some(&[0x68][..]));
    assert_eq!(strip_start_code(&[1, 2, 3]), None);
}

#[rstest]
fn showcase_writes_a_readable_mp4() {
    let directory = std::env::temp_dir().join(format!("dcc-cua-showcase-{}", uuid::Uuid::new_v4()));
    let path = directory.join("showcase.mp4");
    let (sender, receiver) = sync_channel(2);
    let first_captured_at = std::time::Instant::now();
    for sequence in 1..=2 {
        sender
            .send(Arc::new(LiveObservationFrame::new(
                sequence,
                vec![sequence as u8; 16 * 16 * 4],
                16,
                16,
                first_captured_at + std::time::Duration::from_millis((sequence - 1) * 250),
            )))
            .unwrap();
    }
    drop(sender);
    let (ready, _) = oneshot::channel();
    let result = encode_frames(receiver, &path, 10, ready).unwrap();
    assert_eq!(result["finalized"], true);
    assert_eq!(result["duration_ms"], 500);

    let file = File::open(&path).unwrap();
    let size = file.metadata().unwrap().len();
    let mut reader = mp4::Mp4Reader::read_header(BufReader::new(file), size).unwrap();
    assert_eq!(reader.tracks().len(), 1);
    assert_eq!(reader.tracks()[&1].sample_count(), 2);
    assert_eq!(reader.read_sample(1, 1).unwrap().unwrap().duration, 250);
    assert_eq!(reader.read_sample(1, 2).unwrap().unwrap().start_time, 250);
    std::fs::remove_dir_all(directory).unwrap();
}
