//! The run lifecycle states and the one pure rule that decides whether an
//! interrupt renders a terminal frame.
//!
//! A run is [`SessionState::Alive`], then [`SessionState::Closing`] once
//! cancel or close is requested (outstanding effects are being answered or
//! dropped), then [`SessionState::Closed`] once `Run` reports `Done`. An
//! interrupt and a genuine terminal outcome can cross: the operator cancels
//! just as the program returns, or a deadline fires on a run that already
//! failed. [`effective_interrupt`] settles the race with one rule: whichever
//! arrived first is the run's terminal, and the synthetic frame for an
//! interrupt is rendered by [`SyntheticTerminal::message`] and nowhere else.

use super::RunCompletion;

/// Where one run stands between its launch and its final `Done`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// The run is stepping and its effects are being performed.
    Alive,
    /// Cancel or close was requested: the engine's cancel flag is set,
    /// outstanding effects are being answered or dropped, and `Run` has
    /// not yet reported `Done`.
    Closing,
    /// `Run` reported `Done`; nothing is outstanding.
    Closed,
}

impl SessionState {
    /// The state after cancel or close is requested. A closed run stays
    /// closed: a late request has nothing left to interrupt.
    #[must_use]
    pub fn interrupted(self) -> Self {
        match self {
            Self::Alive | Self::Closing => Self::Closing,
            Self::Closed => Self::Closed,
        }
    }

    /// The state after `Run` reports `Done`, whatever preceded it.
    #[must_use]
    pub fn done(self) -> Self {
        match self {
            Self::Alive | Self::Closing | Self::Closed => Self::Closed,
        }
    }
}

/// Why a run is being cut short from outside the program. A session close
/// reaches the run as [`Interrupt::Cancel`]; the reducer's `Close` effect
/// is what distinguishes it at the session level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interrupt {
    /// The operator cancelled the turn.
    Cancel,
    /// The turn's deadline elapsed.
    Timeout,
}

/// The one synthetic terminal frame an interrupt renders when it is the
/// run's effective terminal. Constructed only by [`effective_interrupt`]
/// (and by this module's tests), so a frame in hand means the rule already
/// decided the interrupt won.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticTerminal {
    interrupt: Interrupt,
}

impl SyntheticTerminal {
    /// The frame for `interrupt`. Private so that [`effective_interrupt`]
    /// is structurally the only production path to a frame.
    #[must_use]
    const fn new(interrupt: Interrupt) -> Self {
        Self { interrupt }
    }

    /// The interrupt this frame stands in for.
    #[must_use]
    pub const fn interrupt(self) -> Interrupt {
        self.interrupt
    }

    /// The operator-facing text of the frame: the single place an
    /// interrupt's terminal wording lives.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self.interrupt {
            Interrupt::Cancel => "the agent run was interrupted",
            Interrupt::Timeout => "the agent run timed out",
        }
    }
}

/// What an interrupt does to the run's terminal frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveInterrupt {
    /// A genuine terminal outcome arrived first. The interrupt is late:
    /// it renders nothing, and the outcome already seen stays the run's
    /// terminal.
    Superseded,
    /// The interrupt is the run's terminal; render this frame once. The
    /// `Interrupted` completion the engine reports afterwards adds
    /// nothing.
    Terminal(SyntheticTerminal),
}

/// Settles the race between an interrupt and a genuine terminal outcome:
/// `saw_terminal` is whether a genuine outcome (see
/// [`RunCompletion::is_genuine`]) has already been observed for this run.
#[must_use]
pub fn effective_interrupt(interrupt: Interrupt, saw_terminal: bool) -> EffectiveInterrupt {
    if saw_terminal {
        EffectiveInterrupt::Superseded
    } else {
        EffectiveInterrupt::Terminal(SyntheticTerminal::new(interrupt))
    }
}

impl RunCompletion {
    /// Whether this completion is a genuine terminal outcome of the
    /// program, as opposed to the echo of an interrupt the session itself
    /// requested.
    #[must_use]
    pub fn is_genuine(self) -> bool {
        match self {
            Self::Completed | Self::Failed => true,
            Self::Interrupted => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition::RunCompletion;

    /// The fixture every interrupt-coverage test walks. The match in
    /// [`fixture_index`] is wildcard-free, so adding an `Interrupt` variant
    /// without extending it fails to compile, and extending it without
    /// placing the variant at the returned index fails the coverage test.
    const EVERY_INTERRUPT: [Interrupt; 2] = [Interrupt::Cancel, Interrupt::Timeout];

    /// Where `interrupt` sits in [`EVERY_INTERRUPT`], by declaration.
    fn fixture_index(interrupt: Interrupt) -> usize {
        match interrupt {
            Interrupt::Cancel => 0,
            Interrupt::Timeout => 1,
        }
    }

    /// One thing the session observes about a run, in arrival order.
    #[derive(Clone, Copy, Debug)]
    enum Observed {
        Run(RunCompletion),
        Interrupt(Interrupt),
    }

    /// Folds an arrival order through the rule and collects every
    /// synthetic frame it renders.
    fn frames(observed: &[Observed]) -> Vec<SyntheticTerminal> {
        let mut saw_terminal = false;
        let mut rendered = Vec::new();
        for item in observed {
            match *item {
                Observed::Run(completion) => saw_terminal |= completion.is_genuine(),
                Observed::Interrupt(interrupt) => {
                    match effective_interrupt(interrupt, saw_terminal) {
                        EffectiveInterrupt::Superseded => {}
                        EffectiveInterrupt::Terminal(frame) => rendered.push(frame),
                    }
                }
            }
        }
        rendered
    }

    struct Ordering {
        name: &'static str,
        observed: Vec<Observed>,
        frames: Vec<SyntheticTerminal>,
    }

    #[test]
    fn a_genuine_terminal_before_a_late_interrupt_wins() {
        let cases = vec![
            Ordering {
                name: "completed then late cancel renders nothing",
                observed: vec![
                    Observed::Run(RunCompletion::Completed),
                    Observed::Interrupt(Interrupt::Cancel),
                ],
                frames: vec![],
            },
            Ordering {
                name: "failed then late timeout renders nothing",
                observed: vec![
                    Observed::Run(RunCompletion::Failed),
                    Observed::Interrupt(Interrupt::Timeout),
                ],
                frames: vec![],
            },
            Ordering {
                name: "completed then cancel and timeout both render nothing",
                observed: vec![
                    Observed::Run(RunCompletion::Completed),
                    Observed::Interrupt(Interrupt::Cancel),
                    Observed::Interrupt(Interrupt::Timeout),
                ],
                frames: vec![],
            },
        ];
        for case in cases {
            assert_eq!(frames(&case.observed), case.frames, "{}", case.name);
        }
    }

    #[test]
    fn an_interrupt_before_the_run_ends_renders_its_frame_exactly_once() {
        let cases = vec![
            Ordering {
                name: "cancel then the interrupted completion renders one cancel frame",
                observed: vec![
                    Observed::Interrupt(Interrupt::Cancel),
                    Observed::Run(RunCompletion::Interrupted),
                ],
                frames: vec![SyntheticTerminal::new(Interrupt::Cancel)],
            },
            Ordering {
                name: "timeout then the interrupted completion renders one timeout frame",
                observed: vec![
                    Observed::Interrupt(Interrupt::Timeout),
                    Observed::Run(RunCompletion::Interrupted),
                ],
                frames: vec![SyntheticTerminal::new(Interrupt::Timeout)],
            },
            Ordering {
                name: "an interrupted completion is not genuine, so a later cancel still renders",
                observed: vec![
                    Observed::Run(RunCompletion::Interrupted),
                    Observed::Interrupt(Interrupt::Cancel),
                ],
                frames: vec![SyntheticTerminal::new(Interrupt::Cancel)],
            },
        ];
        for case in cases {
            assert_eq!(frames(&case.observed), case.frames, "{}", case.name);
        }
    }

    #[test]
    fn every_interrupt_variant_renders_before_a_terminal_and_yields_after_one() {
        for interrupt in EVERY_INTERRUPT {
            let index = fixture_index(interrupt);
            assert_eq!(
                EVERY_INTERRUPT.get(index).copied(),
                Some(interrupt),
                "{interrupt:?} is missing from the fixture at index {index}"
            );
            match effective_interrupt(interrupt, false) {
                EffectiveInterrupt::Terminal(frame) => {
                    assert_eq!(
                        frame.interrupt(),
                        interrupt,
                        "{interrupt:?} keeps its cause"
                    );
                    assert!(
                        !frame.message().is_empty(),
                        "{interrupt:?} renders a non-empty frame"
                    );
                }
                EffectiveInterrupt::Superseded => {
                    panic!("{interrupt:?} must render when no terminal was seen")
                }
            }
            assert_eq!(
                effective_interrupt(interrupt, true),
                EffectiveInterrupt::Superseded,
                "{interrupt:?} yields to a genuine terminal"
            );
        }
        let messages: std::collections::BTreeSet<&str> = EVERY_INTERRUPT
            .iter()
            .map(|interrupt| SyntheticTerminal::new(*interrupt).message())
            .collect();
        assert_eq!(
            messages.len(),
            EVERY_INTERRUPT.len(),
            "each interrupt variant renders a distinct frame"
        );
    }

    #[test]
    fn only_completed_and_failed_are_genuine_terminals() {
        assert!(RunCompletion::Completed.is_genuine());
        assert!(RunCompletion::Failed.is_genuine());
        assert!(!RunCompletion::Interrupted.is_genuine());
    }

    #[test]
    fn a_run_goes_alive_closing_closed_and_never_back() {
        let cases: [(SessionState, SessionState, SessionState); 3] = [
            (
                SessionState::Alive,
                SessionState::Closing,
                SessionState::Closed,
            ),
            (
                SessionState::Closing,
                SessionState::Closing,
                SessionState::Closed,
            ),
            (
                SessionState::Closed,
                SessionState::Closed,
                SessionState::Closed,
            ),
        ];
        for (start, after_interrupt, after_done) in cases {
            assert_eq!(
                start.interrupted(),
                after_interrupt,
                "{start:?} interrupted"
            );
            assert_eq!(start.done(), after_done, "{start:?} done");
        }
    }
}
