//! Capability-local borrowing of an owner's already-hidden surfaces.
//! A borrow prolongs the actual guards; it never acquires authority to hide peers.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::IndicatorError;

struct Epoch<R> {
    accepting: AtomicBool,
    resources: R,
}

pub(crate) struct Pool<R>(Mutex<Weak<Epoch<R>>>);

impl<R> Default for Pool<R> {
    fn default() -> Self {
        Self(Mutex::new(Weak::new()))
    }
}

impl<R> Pool<R> {
    pub(crate) fn has_resources(&self) -> Result<bool, IndicatorError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| unavailable())?
            .upgrade()
            .is_some())
    }
    pub(crate) fn publish(&self, resources: R) -> Result<Owner<R>, IndicatorError> {
        let mut slot = self.0.lock().map_err(|_| unavailable())?;
        if slot.upgrade().is_some() {
            return Err(unavailable());
        }
        let epoch = Arc::new(Epoch {
            accepting: AtomicBool::new(true),
            resources,
        });
        *slot = Arc::downgrade(&epoch);
        Ok(Owner(epoch))
    }

    pub(crate) fn borrow(&self) -> Result<Option<Borrow<R>>, IndicatorError> {
        let epoch = self.0.lock().map_err(|_| unavailable())?.upgrade();
        Ok(epoch
            .filter(|epoch| epoch.accepting.load(Ordering::Acquire))
            .map(|epoch| Borrow { epoch }))
    }
}

pub(crate) struct Owner<R>(Arc<Epoch<R>>);

impl<R> Drop for Owner<R> {
    fn drop(&mut self) {
        // Existing capture work may drain; no chain of new borrowers prolongs input.
        self.0.accepting.store(false, Ordering::Release);
    }
}

pub(crate) struct Borrow<R> {
    epoch: Arc<Epoch<R>>,
}

impl<R> Borrow<R> {
    pub(crate) fn resources(&self) -> &R {
        &self.epoch.resources
    }
}

fn unavailable() -> IndicatorError {
    IndicatorError::CaptureExclusion("owned input/capture lease is unavailable".into())
}

#[cfg(test)]
mod tests;
