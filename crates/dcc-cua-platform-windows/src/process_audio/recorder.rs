//! Capture owner handed to the existing video encoder; no independent MP4 path.
use super::*;
use dcc_cua_showcase::{
    audio_track::AacTrackBuffer,
    process_audio::{ProcessAudioMux, ProcessAudioSession},
    recorder_audio::{RecorderAudioFactory, RecorderAudioOwner},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PacketLoopEnd {
    Stopped,
    SourceFailed,
}

fn run_packet_loop<T>(
    max_duration: Duration,
    mut cancelled: impl FnMut() -> bool,
    mut receive: impl FnMut() -> Result<T, mpsc::RecvTimeoutError>,
    mut authorized: impl FnMut() -> bool,
    mut elapsed: impl FnMut() -> Duration,
    mut ingest: impl FnMut(T) -> Result<(), AudioLifecycleError>,
    mut duration_stop_attested: impl FnMut() -> bool,
) -> PacketLoopEnd {
    while !cancelled() {
        if !authorized() {
            return PacketLoopEnd::SourceFailed;
        }
        if elapsed() >= max_duration {
            return PacketLoopEnd::Stopped;
        }
        let received = receive();
        // Explicit source errors and authorization loss must not be hidden by
        // a simultaneous normal duration boundary.
        if !authorized() {
            return PacketLoopEnd::SourceFailed;
        }
        if matches!(received, Err(mpsc::RecvTimeoutError::Disconnected)) {
            return if elapsed() >= max_duration && duration_stop_attested() {
                PacketLoopEnd::Stopped
            } else {
                PacketLoopEnd::SourceFailed
            };
        }
        if elapsed() >= max_duration {
            return PacketLoopEnd::Stopped;
        }
        if let Ok(packet) = received
            && ingest(packet).is_err()
        {
            return PacketLoopEnd::SourceFailed;
        }
    }
    PacketLoopEnd::Stopped
}

pub struct NativeRecorderAudioFactory {
    pub executable: PathBuf,
    pub grant: ProcessAudioGrant,
}

struct BufferMux(Arc<Mutex<Option<AacTrackBuffer>>>);
impl ProcessAudioMux for BufferMux {
    fn write_pcm(
        &mut self,
        pts_us: u64,
        samples: &[i16],
        _: u64,
    ) -> Result<u64, AudioLifecycleError> {
        self.0
            .lock()
            .map_err(|_| AudioLifecycleError::Writer)?
            .as_mut()
            .ok_or(AudioLifecycleError::Writer)?
            .push(pts_us, samples)?;
        Ok(0)
    }
    fn finalize(&mut self) -> Result<(), AudioLifecycleError> {
        Ok(())
    }
}

struct Owner {
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<AacTrackBuffer, AudioLifecycleError>>>,
    evidence: Arc<Mutex<OwnedAudioWorkerEvidence>>,
}
impl RecorderAudioOwner for Owner {
    fn finish(&mut self) -> Result<AacTrackBuffer, AudioLifecycleError> {
        self.cancelled.store(true, Ordering::Release);
        let handle = self.thread.take().ok_or(AudioLifecycleError::Source)?;
        // Source activation/start waits and owned-child reaping are bounded.
        let result = handle
            .join()
            .map_err(|_| AudioLifecycleError::CleanupUnknown)?;
        let evidence = self
            .evidence
            .lock()
            .map_err(|_| AudioLifecycleError::CleanupUnknown)?;
        if !evidence.cleanup_known() {
            return Err(AudioLifecycleError::CleanupUnknown);
        }
        if evidence.spawned
            && (!evidence.stop_acknowledged
                || evidence.forced_termination
                || evidence.exit_code != Some(0)
                || evidence.failure.is_some())
        {
            return Err(AudioLifecycleError::Source);
        }
        result
    }
    fn evidence(&self) -> serde_json::Value {
        self.evidence
            .lock()
            .ok()
            .map(|e| serde_json::json!(*e))
            .unwrap_or_else(|| serde_json::json!({"cleanup":"unknown"}))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.finish();
        }
    }
}

impl RecorderAudioFactory for NativeRecorderAudioFactory {
    fn grant(&self) -> &ProcessAudioGrant {
        &self.grant
    }
    fn start(
        self: Box<Self>,
        epoch: Instant,
        authorization: Arc<RecordingAuthorization>,
        ended: Box<dyn Fn() + Send>,
    ) -> Result<Box<dyn RecorderAudioOwner>, AudioLifecycleError> {
        self.grant.validate()?;
        let build =
            super::worker::supported_windows_build().map_err(|_| AudioLifecycleError::Source)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let buffer = Arc::new(Mutex::new(Some(AacTrackBuffer::new(self.grant.clone())?)));
        let (source, packets, evidence) = NativeProcessAudioSource::new(
            self.executable.clone(),
            self.grant.clone(),
            authorization.clone(),
            epoch,
            cancel.clone(),
        );
        let packet_evidence = evidence.clone();
        let handle = std::thread::Builder::new()
            .name("dcc-cua-recording-audio".into())
            .spawn(move || {
                let mut session = match ProcessAudioSession::start(
                    source,
                    BufferMux(buffer.clone()),
                    self.grant.clone(),
                    self.grant.target,
                    Some(build),
                    authorization.clone(),
                    epoch,
                ) {
                    Ok(session) => {
                        let _ = ready_tx.send(Ok(()));
                        session
                    }
                    Err(failure) => {
                        let error = if failure.cleanup.cleanup_pending {
                            AudioLifecycleError::CleanupUnknown
                        } else {
                            failure.error
                        };
                        let _ = ready_tx.send(Err(error));
                        ended();
                        return Err(error);
                    }
                };
                let outcome = run_packet_loop(
                    Duration::from_millis(self.grant.max_duration_ms),
                    || cancel.load(Ordering::Acquire),
                    || packets.recv_timeout(Duration::from_millis(10)),
                    || authorization.admit().is_ok(),
                    || epoch.elapsed(),
                    |packet| session.ingest(packet.target, packet.pts_us, &packet.samples),
                    || {
                        packet_evidence.lock().is_ok_and(|e| {
                            e.duration_exhausted && e.stop_acknowledged && e.failure.is_none()
                        })
                    },
                );
                let receipt = if outcome == PacketLoopEnd::SourceFailed {
                    session.source_failed()
                } else {
                    session.stop()
                };
                ended();
                if receipt.cleanup_pending {
                    return Err(AudioLifecycleError::CleanupUnknown);
                }
                if receipt.terminal_error.is_some() {
                    return Err(receipt
                        .terminal_error
                        .unwrap_or(AudioLifecycleError::Source));
                }
                buffer
                    .lock()
                    .map_err(|_| AudioLifecycleError::Writer)?
                    .take()
                    .ok_or(AudioLifecycleError::Writer)
            })
            .map_err(|_| AudioLifecycleError::Source)?;
        let mut owner = Owner {
            cancelled,
            thread: Some(handle),
            evidence,
        };
        match ready_rx.recv_timeout(Duration::from_secs(7)) {
            Ok(Ok(())) => Ok(Box::new(owner)),
            other => {
                let error = other
                    .ok()
                    .and_then(Result::err)
                    .unwrap_or(AudioLifecycleError::Source);
                if owner
                    .finish()
                    .is_err_and(|cleanup| cleanup == AudioLifecycleError::CleanupUnknown)
                {
                    return Err(AudioLifecycleError::CleanupUnknown);
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests;
