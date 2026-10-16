use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{LiveObservationFrame, ShowcaseResult, capture_error, publish_without_overwrite};

/// Disk-streamed sample mapping keeps recording memory independent of its length.
pub(super) struct CaptureManifest {
    writer: BufWriter<File>,
    partial_path: PathBuf,
    path: PathBuf,
    digest: Sha256,
    records: u64,
    frames: u64,
}

impl CaptureManifest {
    pub(super) fn create(video: &Path) -> ShowcaseResult<Self> {
        let stem = video
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("showcase");
        let path = video.with_file_name(format!("{stem}.capture.jsonl"));
        let partial_path = video.with_file_name(format!("{stem}.capture.partial.jsonl"));
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&partial_path)
            .map_err(capture_error)?;
        Ok(Self {
            writer: BufWriter::new(file),
            partial_path,
            path,
            digest: Sha256::new(),
            records: 0,
            frames: 0,
        })
    }

    fn append(&mut self, record: Value) -> ShowcaseResult<()> {
        let mut bytes = serde_json::to_vec(&record).map_err(capture_error)?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes).map_err(capture_error)?;
        self.digest.update(&bytes);
        self.records = self.records.saturating_add(1);
        Ok(())
    }

    pub(super) fn frame(
        &mut self,
        frame: &LiveObservationFrame,
        sample_index: u64,
        segment_index: u32,
        media_start_ms: u64,
        width: u32,
        height: u32,
    ) -> ShowcaseResult<()> {
        if sample_index != self.frames {
            return Err(capture_error(
                "recorded provenance sample indices are not contiguous",
            ));
        }
        let (source_width, source_height) = frame.dimensions();
        self.append(json!({
            "schema": "dcc-cua-recorded-frame-v1", "kind": "frame", "media_sample_index": sample_index,
            "segment_index": segment_index, "media_start_ms": media_start_ms,
            "source_sequence": frame.sequence(), "captured_at_ms": frame.captured_at_ms(),
            "source_width": source_width, "source_height": source_height,
            "encoded_width": width, "encoded_height": height,
            "source_to_encoded_scale": {"x_numerator": width, "x_denominator": source_width,
                "y_numerator": height, "y_denominator": source_height},
            "capture_provenance": frame.provenance(),
        }))?;
        self.frames = self.frames.saturating_add(1);
        Ok(())
    }

    pub(super) fn pause(&mut self, media_end_ms: u64) -> ShowcaseResult<()> {
        self.append(
            json!({"schema": "dcc-cua-recorded-frame-v1", "kind": "pause",
            "media_end_ms": media_end_ms, "next_media_sample_index": self.frames}),
        )?;
        self.writer.flush().map_err(capture_error)
    }

    pub(super) fn finish(mut self, expected_frames: u64) -> ShowcaseResult<Value> {
        if self.frames != expected_frames {
            return Err(capture_error(
                "recorded provenance does not cover every admitted media sample",
            ));
        }
        self.writer.flush().map_err(capture_error)?;
        self.writer.get_ref().sync_all().map_err(capture_error)?;
        drop(self.writer);
        publish_without_overwrite(&self.partial_path, &self.path)?;
        Ok(
            json!({"schema": "dcc-cua-recorded-frame-v1", "path": self.path.to_string_lossy(),
            "sha256": format!("{:x}", self.digest.finalize()), "records": self.records,
            "frames": self.frames, "finalized": true,
            "mapping": "one_record_per_admitted_media_sample_with_explicit_pause_boundaries"}),
        )
    }
}
