//! The anti-flicker policy between the gateway's progress snapshots and
//! the status bar: the barberpole appears only once the gateway has been
//! busy for [`SHOW_DELAY`], stays up at least [`MIN_VISIBLE`] once shown,
//! and follows the newest text while up. The policy lives here, next to
//! the subscriber that feeds it, so the UI stays dumb and the wire type
//! stays a plain busy flag plus text.
//!
//! [`Presenter`] is a pure state machine over explicit instants: the
//! subscriber loop hands it every decoded snapshot and wakes it at
//! [`Presenter::next_wake`], and the tests drive it with instants of
//! their own choosing.

use std::time::Duration;

use gateway_api_types::Progress;
use tokio::time::Instant;

use workshop_protocol::Activity;
use workshop_registry::Push;

/// How long the gateway must stay busy before the barberpole appears;
/// work shorter than this never disturbs the status bar.
pub(crate) const SHOW_DELAY: Duration = Duration::from_secs(1);

/// How long the barberpole stays up once shown, so work that ends just
/// past [`SHOW_DELAY`] reads as a completed activity.
pub(crate) const MIN_VISIBLE: Duration = Duration::from_millis(500);

/// The tooltip on every gateway busy frame.
const DESCRIPTION: &str = "gateway activity";

/// The two anti-flicker durations, injectable so the subscriber tests
/// run against a live mock gateway without waiting out the production
/// values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    /// Busy time before the barberpole appears.
    pub(crate) show_delay: Duration,
    /// Minimum time the barberpole stays up once shown.
    pub(crate) min_visible: Duration,
}

impl Policy {
    /// The production policy.
    pub(crate) const DEFAULT: Self = Self {
        show_delay: SHOW_DELAY,
        min_visible: MIN_VISIBLE,
    };
}

/// The gateway is busy: since when, and its newest text.
#[derive(Debug)]
struct Live {
    since: Instant,
    text: String,
}

/// The barberpole is up: since when, and the text last pushed.
#[derive(Debug)]
struct Shown {
    at: Instant,
    text: String,
}

/// The anti-flicker state machine over gateway snapshots.
#[derive(Debug)]
pub(crate) struct Presenter {
    policy: Policy,
    live: Option<Live>,
    shown: Option<Shown>,
}

impl Presenter {
    /// A presenter at rest: nothing live, nothing shown.
    pub(crate) fn new(policy: Policy) -> Self {
        Self {
            policy,
            live: None,
            shown: None,
        }
    }

    /// Records one decoded snapshot at `now` and pushes whatever
    /// transition it calls for. A busy snapshot starts the show delay
    /// (or replaces the live text); an idle snapshot ends the busy run
    /// and starts the minimum-visible hold if the bar is up.
    pub(crate) fn apply(&mut self, snapshot: Progress, now: Instant, push: &Push) {
        if snapshot.busy {
            match &mut self.live {
                Some(live) => live.text = snapshot.text,
                None => {
                    self.live = Some(Live {
                        since: now,
                        text: snapshot.text,
                    });
                }
            }
        } else {
            self.live = None;
        }
        self.settle(now, push);
    }

    /// Re-evaluates the deadlines at `now` without a new snapshot: the
    /// show delay lapsing pushes the bar up, the minimum-visible hold
    /// lapsing pushes it idle.
    pub(crate) fn tick(&mut self, now: Instant, push: &Push) {
        self.settle(now, push);
    }

    /// The next moment [`Presenter::tick`] can change state without a
    /// snapshot: the show deadline while the gateway warms up, or the
    /// earliest idle moment once the gateway went idle under a shown bar.
    pub(crate) fn next_wake(&self) -> Option<Instant> {
        match (&self.live, &self.shown) {
            (Some(live), None) => Some(live.since + self.policy.show_delay),
            (None, Some(shown)) => Some(shown.at + self.policy.min_visible),
            (Some(_), Some(_)) | (None, None) => None,
        }
    }

    /// The subscription is gone at `now`: progress from a gateway the
    /// workshop can no longer hear is stale, so this reads as an idle
    /// snapshot. Any pending show is forgotten, and a shown bar rests once
    /// its minimum-visible hold has lapsed, the same hold an idle snapshot
    /// gets; the subscriber keeps [`Presenter::next_wake`] armed between
    /// subscriptions so the hold lapses on time.
    pub(crate) fn detach(&mut self, now: Instant, push: &Push) {
        self.live = None;
        self.settle(now, push);
    }

    fn settle(&mut self, now: Instant, push: &Push) {
        let Some(live) = &self.live else {
            if let Some(shown) = &self.shown
                && now.duration_since(shown.at) >= self.policy.min_visible
            {
                self.shown = None;
                push.push_idle();
            }
            return;
        };
        match self.shown.as_mut() {
            None => {
                if now.duration_since(live.since) >= self.policy.show_delay {
                    push.push_busy(live.text.clone(), DESCRIPTION, Activity::General);
                    self.shown = Some(Shown {
                        at: now,
                        text: live.text.clone(),
                    });
                }
            }
            Some(shown) => {
                if shown.text != live.text {
                    push.push_busy(live.text.clone(), DESCRIPTION, Activity::General);
                    shown.text.clone_from(&live.text);
                }
            }
        }
    }
}
