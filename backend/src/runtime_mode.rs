//! Process-wide optimistic mode preconditions. The lock is never held across I/O.
use crate::{
    error::{Error, Result},
    model::RuntimeMode,
};
use std::sync::Mutex;

pub struct Mode {
    state: Mutex<State>,
}

struct State {
    mode: RuntimeMode,
    boot: String,
    generation: u64,
}

fn entropy() -> Result<String> {
    use std::fmt::Write;
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::internal())?;
    let mut token = String::with_capacity(32);
    for byte in bytes {
        write!(token, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(token)
}
fn revision(boot: &str, generation: u64) -> Result<String> {
    // Random per boot AND per toggle, plus a checked generation preventing reuse
    // within this process even in the event of a repeated entropy sample.
    Ok(format!("{boot}-{}-{generation:016x}", entropy()?))
}
fn conflict() -> Error {
    Error::new(
        http::StatusCode::CONFLICT,
        "mode_conflict",
        "Runtime mode changed. Review the current mode and submit again explicitly.",
    )
}
impl Mode {
    pub fn new(read_only_test_mode: bool) -> Result<Self> {
        let boot = entropy()?;
        let mode_revision = revision(&boot, 0)?;
        Ok(Self {
            state: Mutex::new(State {
                mode: RuntimeMode {
                    read_only_test_mode,
                    mode_revision,
                },
                boot,
                generation: 0,
            }),
        })
    }
    pub fn read(&self) -> RuntimeMode {
        self.state.lock().expect("mode lock poisoned").mode.clone()
    }
    /// Compare and capture once, before business validation or any upstream calls.
    /// The captured boolean must be used for every step and follow-up of the action.
    pub fn admit(&self, expected: &str) -> Result<bool> {
        let state = self.state.lock().expect("mode lock poisoned");
        if state.mode.mode_revision != expected {
            return Err(conflict());
        }
        Ok(state.mode.read_only_test_mode)
    }
    pub fn change(&self, expected: &str, read_only_test_mode: bool) -> Result<RuntimeMode> {
        let mut state = self.state.lock().expect("mode lock poisoned");
        if state.mode.mode_revision != expected {
            return Err(conflict());
        }
        if state.mode.read_only_test_mode != read_only_test_mode {
            let generation = state
                .generation
                .checked_add(1)
                .ok_or_else(Error::internal)?;
            let mode_revision = revision(&state.boot, generation)?;
            state.mode = RuntimeMode {
                read_only_test_mode,
                mode_revision,
            };
            state.generation = generation;
        }
        Ok(state.mode.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn racing_toggle_and_admission_are_linearizable_and_captured_modes_are_immutable() {
        for _ in 0..64 {
            let mode = Mode::new(true).unwrap();
            let initial = mode.read();
            let barrier = std::sync::Barrier::new(3);
            std::thread::scope(|scope| {
                let toggle = scope.spawn(|| {
                    barrier.wait();
                    mode.change(&initial.mode_revision, false)
                });
                let admission = scope.spawn(|| {
                    barrier.wait();
                    mode.admit(&initial.mode_revision)
                });
                barrier.wait();
                let live = toggle.join().unwrap().unwrap();
                match admission.join().unwrap() {
                    Ok(captured) => assert!(captured),
                    Err(e) => assert_eq!(e.code, "mode_conflict"),
                }
                let captured_live = mode.admit(&live.mode_revision).unwrap();
                let test = mode.change(&live.mode_revision, true).unwrap();
                assert!(!captured_live);
                assert!(mode.admit(&test.mode_revision).unwrap());
                assert_ne!(test.mode_revision, initial.mode_revision);
            });
        }
    }
}
