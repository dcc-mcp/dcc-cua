use super::*;
use crate::{LiveObservationFrame, LiveObservationStatus, ShowcaseRecorder};
use dcc_cua_protocol::recording_audio::ProcessAudioTarget;
use rstest::rstest;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Factory {
    grant: ProcessAudioGrant,
    stops: Arc<AtomicUsize>,
    fail: bool,
    unknown: bool,
    startup_error: Option<AudioLifecycleError>,
}
struct Owner {
    ended: Option<Box<dyn Fn() + Send>>,
    buffer: Option<AacTrackBuffer>,
    stops: Arc<AtomicUsize>,
    fail: bool,
    unknown: bool,
}
impl RecorderAudioOwner for Owner {
    fn finish(&mut self) -> Result<AacTrackBuffer, AudioLifecycleError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        if let Some(ended) = self.ended.take() {
            ended();
        }
        if self.fail {
            return Err(if self.unknown {
                AudioLifecycleError::CleanupUnknown
            } else {
                AudioLifecycleError::Source
            });
        }
        self.buffer.take().ok_or(AudioLifecycleError::Source)
    }
    fn evidence(&self) -> serde_json::Value {
        serde_json::json!({"synthetic":true,"native_capture":false})
    }
}
impl RecorderAudioFactory for Factory {
    fn grant(&self) -> &ProcessAudioGrant {
        &self.grant
    }
    fn start(
        self: Box<Self>,
        _: Instant,
        _: Arc<RecordingAuthorization>,
        ended: Box<dyn Fn() + Send>,
    ) -> Result<Box<dyn RecorderAudioOwner>, AudioLifecycleError> {
        if let Some(error) = self.startup_error {
            return Err(error);
        }
        let mut buffer = AacTrackBuffer::new(self.grant)?;
        let samples: Vec<i16> = (0..1920)
            .map(|index| if index % 48 < 24 { 12000 } else { -12000 })
            .collect();
        buffer.push(50_000, &samples)?;
        Ok(Box::new(Owner {
            ended: Some(ended),
            buffer: Some(buffer),
            stops: self.stops,
            fail: self.fail,
            unknown: self.unknown,
        }))
    }
}

#[rstest]
#[case(false, false)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn existing_recorder_muxes_audio_and_never_publishes_failed_source_as_silent(
    #[case] fail: bool,
    #[case] unknown: bool,
) {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-owned-audio-mux-{}", uuid::Uuid::new_v4()));
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![80; 16 * 16 * 4], 16, 16, Instant::now()),
        std::time::Duration::ZERO,
        "generated",
    );
    let (_sender, receiver) = tokio::sync::watch::channel(status);
    let stops = Arc::new(AtomicUsize::new(0));
    let factory = Factory {
        grant: ProcessAudioGrant {
            target: ProcessAudioTarget {
                process_id: 1,
                process_creation_time_100ns: 2,
                window_handle: 3,
                window_thread_id: 4,
                window_class_hash: 5,
            },
            max_duration_ms: 1000,
            max_output_bytes: 1_000_000,
        },
        stops: stops.clone(),
        fail,
        unknown,
        startup_error: None,
    };
    let authority = Arc::new(RecordingAuthorization::new(u64::MAX));
    let recorder = ShowcaseRecorder::start_with_audio_outcome(
        receiver,
        directory.to_str().unwrap(),
        10,
        Some(authority),
        Some(Box::new(factory)),
    )
    .await
    .map_err(|(e, _)| e)
    .unwrap();
    let (result, outcome) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        recorder.stop_with_outcome(),
    )
    .await
    .expect("audio callback must not retain producer channel and deadlock stop");
    assert_eq!(stops.load(Ordering::SeqCst), 1);
    if fail {
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            if unknown {
                crate::ShowcaseErrorCode::CleanupUnknown
            } else {
                crate::ShowcaseErrorCode::CaptureFailed
            }
        );
        assert_eq!(outcome["finalized"], false);
        assert!(!directory.join("showcase.mp4").exists());
    } else {
        let state = result.unwrap();
        assert_eq!(state["finalized"], true);
        assert_eq!(state["audio"]["worker"]["native_capture"], false);
        assert!(
            state["audio"]["codec"]["nonzero_input_samples"]
                .as_u64()
                .unwrap()
                > 0
        );
        let file = std::fs::File::open(directory.join("showcase.mp4")).unwrap();
        let size = file.metadata().unwrap().len();
        let reader = mp4::Mp4Reader::read_header(file, size).unwrap();
        assert_eq!(reader.tracks().len(), 2);
    }
    // Only this freshly UUID-created generated-file directory; no OS fixtures.
    std::fs::remove_dir_all(directory).unwrap();
}

fn generated_factory(startup_error: Option<AudioLifecycleError>) -> Factory {
    Factory {
        grant: ProcessAudioGrant {
            target: ProcessAudioTarget {
                process_id: 1,
                process_creation_time_100ns: 2,
                window_handle: 3,
                window_thread_id: 4,
                window_class_hash: 5,
            },
            max_duration_ms: 1000,
            max_output_bytes: 1_000_000,
        },
        stops: Arc::new(AtomicUsize::new(0)),
        fail: false,
        unknown: false,
        startup_error,
    }
}

#[rstest]
#[tokio::test]
async fn startup_cleanup_unknown_reaches_caller_without_generic_error_substitution() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-audio-start-{}", uuid::Uuid::new_v4()));
    let mut status = LiveObservationStatus::default();
    status.publish_frame(
        LiveObservationFrame::new(1, vec![80; 16 * 16 * 4], 16, 16, Instant::now()),
        std::time::Duration::ZERO,
        "generated",
    );
    let (_sender, receiver) = tokio::sync::watch::channel(status);
    let result = ShowcaseRecorder::start_with_audio_outcome(
        receiver,
        directory.to_str().unwrap(),
        10,
        Some(Arc::new(RecordingAuthorization::new(u64::MAX))),
        Some(Box::new(generated_factory(Some(
            AudioLifecycleError::CleanupUnknown,
        )))),
    )
    .await;
    let (error, outcome) = result.err().expect("startup must fail");
    assert_eq!(error.code, crate::ShowcaseErrorCode::CleanupUnknown);
    assert_eq!(
        error.message,
        AudioLifecycleError::CleanupUnknown.to_string()
    );
    assert_eq!(outcome["finalized"], false);
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
fn audio_end_is_observed_even_when_its_wakeup_cannot_enter_the_full_frame_queue() {
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-audio-end-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    for sequence in 1..=2 {
        sender
            .try_send(crate::ShowcaseProducerEvent::Frame(Arc::new(
                LiveObservationFrame::new(sequence, vec![80; 16 * 16 * 4], 16, 16, Instant::now()),
            )))
            .unwrap();
    }
    let ended = Arc::new(crate::AtomicBool::new(false));
    let callback = crate::audio_ended_callback(sender.downgrade(), ended.clone());
    callback();
    assert_eq!(sender.capacity(), 0, "wakeup could not be enqueued");
    drop(sender);
    let authority = Arc::new(RecordingAuthorization::new(u64::MAX));
    let terminal = std::sync::Mutex::new(None);
    let (ready, _acknowledged) = tokio::sync::oneshot::channel();
    let result = crate::encode_frames_with_audio(
        receiver,
        &directory.join("showcase.mp4"),
        10,
        ready,
        &std::sync::Mutex::new(crate::ShowcaseProgress::default()),
        crate::EncoderAuthorization {
            video: Some(&authority),
            audio: Some(crate::AudioRun {
                factory: Box::new(generated_factory(None)),
                authorization: authority.clone(),
                ended: callback,
                ended_flag: ended,
            }),
        },
        &terminal,
    )
    .unwrap();
    assert_eq!(
        result["frames"], 1,
        "no queued frame may pass the ended fence"
    );
    assert_eq!(
        terminal.lock().unwrap().as_ref().unwrap()["code"],
        "recording_audio_ended"
    );
    std::fs::remove_dir_all(directory).unwrap();
}
