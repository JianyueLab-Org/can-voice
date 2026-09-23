use can_voice_fsd::session::{FsdEvent, FsdState, Reason};
use std::sync::Mutex;
use tokio::sync::{watch, Mutex as AsyncMutex};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Connecting,
    Active,
    Stopping,
}

struct State {
    generation: u64,
    phase: Phase,
}

/// Serializes startup/teardown while a generation invalidates in-flight awaits.
pub struct LifeGate {
    state: Mutex<State>,
    pub operation: AsyncMutex<()>,
    cancel: watch::Sender<u64>,
    stopped: watch::Sender<u64>,
}

impl LifeGate {
    pub fn new() -> Self {
        let (cancel, _) = watch::channel(0);
        let (stopped, _) = watch::channel(0);
        Self {
            state: Mutex::new(State {
                generation: 0,
                phase: Phase::Idle,
            }),
            operation: AsyncMutex::new(()),
            cancel,
            stopped,
        }
    }

    pub fn begin(&self) -> Option<(u64, watch::Receiver<u64>)> {
        let mut state = self.state.lock().expect("lifecycle");
        if state.phase != Phase::Idle {
            return None;
        }
        state.generation += 1;
        state.phase = Phase::Connecting;
        Some((state.generation, self.cancel.subscribe()))
    }

    pub fn current(&self, generation: u64) -> bool {
        let state = self.state.lock().expect("lifecycle");
        state.generation == generation && state.phase == Phase::Connecting
    }

    pub fn activate(&self, generation: u64) -> bool {
        let mut state = self.state.lock().expect("lifecycle");
        if state.generation != generation || state.phase != Phase::Connecting {
            return false;
        }
        state.phase = Phase::Active;
        true
    }

    pub fn active(&self, generation: u64) -> bool {
        let state = self.state.lock().expect("lifecycle");
        state.generation == generation && state.phase == Phase::Active
    }

    pub fn fail_connect(&self, generation: u64) {
        let mut state = self.state.lock().expect("lifecycle");
        if state.generation == generation && state.phase == Phase::Connecting {
            state.phase = Phase::Idle;
        }
    }

    /// `Some(id)` is reserved for the reader of that active session.
    pub fn request_stop(&self, expected: Option<u64>) -> Option<u64> {
        let mut state = self.state.lock().expect("lifecycle");
        if state.phase == Phase::Stopping {
            return None;
        }
        if let Some(expected) = expected {
            if state.generation != expected || state.phase != Phase::Active {
                return None;
            }
        }
        state.generation += 1;
        state.phase = Phase::Stopping;
        self.cancel.send_replace(state.generation);
        Some(state.generation)
    }

    pub fn finish_stop(&self, generation: u64) {
        let mut state = self.state.lock().expect("lifecycle");
        if state.generation == generation && state.phase == Phase::Stopping {
            state.phase = Phase::Idle;
            self.stopped.send_replace(generation);
        }
    }

    pub async fn wait_for_stop(&self) {
        let mut stopped = self.stopped.subscribe();
        let generation = {
            let state = self.state.lock().expect("lifecycle");
            if state.phase != Phase::Stopping {
                return;
            }
            state.generation
        };
        while *stopped.borrow_and_update() < generation {
            if stopped.changed().await.is_err() {
                return;
            }
        }
    }
}

pub fn terminal_fsd(event: &FsdEvent) -> bool {
    matches!(
        (&event.state, &event.reason),
        (FsdState::Stopped, Reason::Stopped) | (FsdState::Offline, Reason::GaveUp { .. })
    )
}

#[cfg(test)]
mod tests {
    use super::LifeGate;
    use std::sync::Arc;

    #[tokio::test]
    async fn concurrent_disconnect_waits_for_existing_teardown() {
        let gate = Arc::new(LifeGate::new());
        let (active, _) = gate.begin().expect("connect");
        assert!(gate.activate(active));
        let stopping = gate.request_stop(None).expect("first disconnect");
        assert!(gate.request_stop(None).is_none());
        let waiting = tokio::spawn({
            let gate = gate.clone();
            async move { gate.wait_for_stop().await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        gate.finish_stop(stopping);
        waiting.await.expect("second disconnect resumes");
    }
}
