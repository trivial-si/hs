//! Pomodoro timing and ownership. Each session carries exactly the data required by its state.
use crate::{
    Error, Result,
    model::ActivityTemplate,
    time::{Moment, local_at},
};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub focus_seconds: i64,
    pub break_seconds: i64,
    pub long_break_seconds: i64,
    pub rounds: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            focus_seconds: 25 * 60,
            break_seconds: 5 * 60,
            long_break_seconds: 15 * 60,
            rounds: 4,
        }
    }
}

impl Config {
    pub(crate) fn validate(self, now: &Moment) -> Result<()> {
        self.validate_values()?;
        for seconds in [
            self.focus_seconds,
            self.break_seconds,
            self.long_break_seconds,
        ] {
            deadline(now, seconds)?;
        }
        Ok(())
    }

    fn validate_values(self) -> Result<()> {
        if self.rounds == 0 {
            return Err(Error::Input(
                "The long-break interval must be at least one focus period".into(),
            ));
        }
        for seconds in [
            self.focus_seconds,
            self.break_seconds,
            self.long_break_seconds,
        ] {
            if seconds <= 0 {
                return Err(Error::Input("Pomodoro durations must be positive".into()));
            }
        }
        Ok(())
    }

    pub(crate) fn duration(self, phase: Phase) -> i64 {
        match phase {
            Phase::Focus => self.focus_seconds,
            Phase::ShortBreak => self.break_seconds,
            Phase::LongBreak => self.long_break_seconds,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}
impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::ShortBreak => "short break",
            Self::LongBreak => "long break",
        }
    }
    /// Stable phase name used by SQLite and the JSON output contract.
    pub fn key(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::ShortBreak => "short_break",
            Self::LongBreak => "long_break",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "focus" => Ok(Self::Focus),
            "short_break" => Ok(Self::ShortBreak),
            "long_break" => Ok(Self::LongBreak),
            _ => Err(Error::Input("Invalid saved Pomodoro phase".into())),
        }
    }
}

/// Persisted lifecycle of one phase, independent of the session's offered next phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseState {
    Running,
    Paused,
    Completed,
    Interrupted,
}

impl PhaseState {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(Error::Input("Invalid saved Pomodoro phase state".into())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchedPhase {
    pub id: i64,
    pub session_id: i64,
    pub phase: Phase,
    pub state: PhaseState,
    pub ended_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Paused,
    Ready,
    Stopped,
}
impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Ready => "ready",
            Self::Stopped => "stopped",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "ready" => Ok(Self::Ready),
            "stopped" => Ok(Self::Stopped),
            _ => Err(Error::Input("Invalid saved Pomodoro state".into())),
        }
    }
}

pub(crate) fn deadline(now: &Moment, seconds: i64) -> Result<DateTime<Utc>> {
    let end = Duration::try_seconds(seconds)
        .and_then(|duration| now.utc.checked_add_signed(duration))
        .ok_or_else(|| Error::Input("Pomodoro duration is outside the supported range".into()))?;
    // Completing a focus must also be able to write its local end time.
    crate::model::validate_civil_time(local_at(end, now.zone)?)?;
    Ok(end)
}

/// Fields read together when restoring a session. The adapter may receive a
/// legacy row in which deleting the work entry cleared its foreign key.
pub(crate) struct SavedTimer {
    pub phase: Phase,
    pub state: State,
    pub deadline: Option<DateTime<Utc>>,
    pub remaining: i64,
    pub entry_id: Option<i64>,
    pub phase_id: Option<i64>,
    pub config: Config,
    pub completed: u32,
    pub interrupted: u32,
    pub zone: Tz,
    pub last_observed: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
enum RunningWork {
    Focus {
        entry_id: i64,
    },
    ShortBreak,
    LongBreak,
    /// Reconciliation interrupts this phase before exposing a live session.
    LostFocus,
}

impl RunningWork {
    fn phase(self) -> Phase {
        match self {
            Self::Focus { .. } | Self::LostFocus => Phase::Focus,
            Self::ShortBreak => Phase::ShortBreak,
            Self::LongBreak => Phase::LongBreak,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Timer {
    Running {
        work: RunningWork,
        deadline: DateTime<Utc>,
        phase_id: i64,
    },
    Paused {
        phase: Phase,
        remaining: i64,
        phase_id: i64,
    },
    Ready {
        next: Phase,
        previous_id: Option<i64>,
    },
    Stopped {
        phase: Phase,
        previous_id: Option<i64>,
    },
}

impl Timer {
    fn restore(saved: &SavedTimer) -> Result<Self> {
        let duration = saved.config.duration(saved.phase);
        let invalid = || {
            Error::Conflict("Inconsistent saved Pomodoro state: check its deadline, phase, work entry, and remaining time".into())
        };
        match (saved.state, saved.deadline, saved.phase_id, saved.entry_id) {
            (State::Running, Some(deadline), Some(phase_id), entry)
                if (1..=duration).contains(&saved.remaining) =>
            {
                let work = match (saved.phase, entry) {
                    (Phase::Focus, Some(entry_id)) => RunningWork::Focus { entry_id },
                    (Phase::Focus, None) => RunningWork::LostFocus,
                    (Phase::ShortBreak, None) => RunningWork::ShortBreak,
                    (Phase::LongBreak, None) => RunningWork::LongBreak,
                    (_, Some(_)) => return Err(invalid()),
                };
                Ok(Self::Running {
                    work,
                    deadline,
                    phase_id,
                })
            }
            (State::Paused, None, Some(phase_id), None)
                if (1..=duration).contains(&saved.remaining) =>
            {
                Ok(Self::Paused {
                    phase: saved.phase,
                    remaining: saved.remaining,
                    phase_id,
                })
            }
            (State::Ready, None, previous_id, None) if saved.remaining == duration => {
                Ok(Self::Ready {
                    next: saved.phase,
                    previous_id,
                })
            }
            (State::Stopped, None, previous_id, None) if saved.remaining == 0 => {
                Ok(Self::Stopped {
                    phase: saved.phase,
                    previous_id,
                })
            }
            _ => Err(invalid()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: i64,
    pub task_id: Option<i64>,
    pub activity: String,
    pub category: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    timer: Timer,
    config: Config,
    completed_focus: u32,
    interrupted_focus: u32,
    zone: Tz,
    last_observed: DateTime<Utc>,
}

/// The decision uses the clock and current owner supplied by the transaction.
/// The caller persists it in the same transaction that observed those inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tick {
    Idle,
    Observe(DateTime<Utc>),
    InterruptLostFocus,
    Complete(Completion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Completion {
    pub phase_id: i64,
    pub at: DateTime<Utc>,
    pub observed: DateTime<Utc>,
    pub next: Phase,
    pub completed: u32,
}

pub(crate) struct Interruption {
    pub at: DateTime<Utc>,
    pub phase_id: Option<i64>,
    pub remaining: i64,
    pub interrupted_focus: u32,
}

#[derive(Debug)]
pub struct Statistics {
    pub completed: i64,
    pub interrupted: i64,
    pub focus_seconds: i64,
    pub break_seconds: i64,
}

impl Session {
    pub(crate) fn restore(
        id: i64,
        task_id: Option<i64>,
        details: ActivityTemplate,
        saved: SavedTimer,
    ) -> Result<Self> {
        saved.config.validate_values()?;
        let timer = Timer::restore(&saved)?;
        Ok(Self {
            id,
            task_id,
            activity: details.activity,
            category: details.category,
            description: details.description,
            tags: details.tags,
            timer,
            config: saved.config,
            completed_focus: saved.completed,
            interrupted_focus: saved.interrupted,
            zone: saved.zone,
            last_observed: saved.last_observed,
        })
    }

    pub fn state(&self) -> State {
        match self.timer {
            Timer::Running { .. } => State::Running,
            Timer::Paused { .. } => State::Paused,
            Timer::Ready { .. } => State::Ready,
            Timer::Stopped { .. } => State::Stopped,
        }
    }

    pub fn phase(&self) -> Phase {
        match self.timer {
            Timer::Running { work, .. } => work.phase(),
            Timer::Paused { phase, .. } | Timer::Stopped { phase, .. } => phase,
            Timer::Ready { next, .. } => next,
        }
    }

    pub fn entry_id(&self) -> Option<i64> {
        match self.timer {
            Timer::Running {
                work: RunningWork::Focus { entry_id },
                ..
            } => Some(entry_id),
            _ => None,
        }
    }

    pub fn phase_id(&self) -> Option<i64> {
        match self.timer {
            Timer::Running { phase_id, .. } | Timer::Paused { phase_id, .. } => Some(phase_id),
            Timer::Ready { previous_id, .. } | Timer::Stopped { previous_id, .. } => previous_id,
        }
    }

    pub fn deadline(&self) -> Option<DateTime<Utc>> {
        match self.timer {
            Timer::Running { deadline, .. } => Some(deadline),
            _ => None,
        }
    }

    pub fn last_observed(&self) -> DateTime<Utc> {
        self.last_observed
    }
    pub fn completed_focus(&self) -> u32 {
        self.completed_focus
    }
    pub fn interrupted_focus(&self) -> u32 {
        self.interrupted_focus
    }
    pub fn config(&self) -> Config {
        self.config
    }
    pub fn zone(&self) -> Tz {
        self.zone
    }

    pub(crate) fn observed(&self, now: &Moment) -> DateTime<Utc> {
        now.utc.max(self.last_observed)
    }
    pub(crate) fn duration(&self) -> i64 {
        self.config.duration(self.phase())
    }

    pub fn remaining(&self, now: &Moment) -> i64 {
        self.remaining_at(self.observed(now))
    }

    pub(crate) fn remaining_at(&self, at: DateTime<Utc>) -> i64 {
        match self.timer {
            Timer::Running { deadline, .. } => (deadline - at).num_seconds().max(0),
            Timer::Paused { remaining, .. } => remaining,
            Timer::Ready { next, .. } => self.config.duration(next),
            Timer::Stopped { .. } => 0,
        }
    }

    pub(crate) fn paused(&self) -> Option<(i64, i64)> {
        match self.timer {
            Timer::Paused {
                phase_id,
                remaining,
                ..
            } => Some((phase_id, remaining)),
            _ => None,
        }
    }

    pub(crate) fn interruption(
        &self,
        now: &Moment,
        explicit: Option<DateTime<Utc>>,
    ) -> Result<Interruption> {
        let observed = self.observed(now);
        let (at, phase_id, focus) = match self.timer {
            Timer::Running { work, phase_id, .. } if work.phase() == Phase::Focus => {
                (explicit.unwrap_or(observed), Some(phase_id), true)
            }
            Timer::Running { phase_id, .. } => (observed, Some(phase_id), false),
            Timer::Paused {
                phase_id, phase, ..
            } => (observed, Some(phase_id), phase == Phase::Focus),
            Timer::Ready { .. } | Timer::Stopped { .. } => (observed, None, false),
        };
        let interrupted_focus = self
            .interrupted_focus
            .checked_add(u32::from(focus))
            .ok_or_else(|| {
                Error::Conflict("Interrupted Pomodoro count exceeds the supported range".into())
            })?;
        Ok(Interruption {
            at,
            phase_id,
            remaining: self.remaining_at(at),
            interrupted_focus,
        })
    }

    pub(crate) fn tick(&self, now: &Moment, current_entry: Option<i64>) -> Result<Tick> {
        let Timer::Running {
            work,
            deadline,
            phase_id,
        } = self.timer
        else {
            return Ok(Tick::Idle);
        };
        match work {
            RunningWork::LostFocus => return Ok(Tick::InterruptLostFocus),
            RunningWork::Focus { entry_id } if current_entry != Some(entry_id) => {
                return Ok(Tick::InterruptLostFocus);
            }
            _ => {}
        }
        let observed = self.observed(now);
        if observed < deadline {
            return Ok(Tick::Observe(observed));
        }
        let completed = self
            .completed_focus
            .checked_add(u32::from(work.phase() == Phase::Focus))
            .ok_or_else(|| {
                Error::Conflict("Completed Pomodoro count exceeds the supported range".into())
            })?;
        let next = match work.phase() {
            Phase::Focus if completed % self.config.rounds == 0 => Phase::LongBreak,
            Phase::Focus => Phase::ShortBreak,
            Phase::ShortBreak | Phase::LongBreak => Phase::Focus,
        };
        Ok(Tick::Complete(Completion {
            phase_id,
            at: deadline,
            observed,
            next,
            completed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(seconds: i64) -> Moment {
        Moment::at(
            DateTime::from_timestamp(1_750_000_000 + seconds, 0).unwrap(),
            chrono_tz::UTC,
        )
    }

    fn session(phase: Phase, completed: u32, owner: Option<i64>) -> Session {
        let config = Config {
            focus_seconds: 30,
            break_seconds: 5,
            long_break_seconds: 10,
            rounds: 2,
        };
        let duration = config.duration(phase);
        Session::restore(
            1,
            None,
            ActivityTemplate {
                activity: "Work".into(),
                category: None,
                description: String::new(),
                tags: vec![],
            },
            SavedTimer {
                phase,
                state: State::Running,
                deadline: Some(instant(duration).utc),
                remaining: duration,
                entry_id: owner,
                phase_id: Some(7),
                config,
                completed,
                interrupted: 0,
                zone: chrono_tz::UTC,
                last_observed: instant(3).utc,
            },
        )
        .unwrap()
    }

    #[test]
    fn clock_and_ownership_decide_exactly_one_transition() {
        for (phase, completed, next, expected_count) in [
            (Phase::Focus, 0, Phase::ShortBreak, 1),
            (Phase::Focus, 1, Phase::LongBreak, 2),
            (Phase::ShortBreak, 1, Phase::Focus, 1),
            (Phase::LongBreak, 2, Phase::Focus, 2),
        ] {
            let owner = (phase == Phase::Focus).then_some(10);
            let session = session(phase, completed, owner);
            let duration = session.duration();
            assert_eq!(
                session.tick(&instant(1), owner).unwrap(),
                Tick::Observe(instant(3).utc)
            );
            assert_eq!(session.remaining(&instant(1)), duration - 3);
            let late = instant(duration + 60);
            assert_eq!(
                session.tick(&late, owner).unwrap(),
                Tick::Complete(Completion {
                    phase_id: 7,
                    at: instant(duration).utc,
                    observed: late.utc,
                    next,
                    completed: expected_count
                })
            );
            if phase == Phase::Focus {
                assert_eq!(
                    session.tick(&late, Some(12)).unwrap(),
                    Tick::InterruptLostFocus
                );
            }
        }
        let missing = session(Phase::Focus, 0, None);
        assert_eq!(
            missing.tick(&instant(30), None).unwrap(),
            Tick::InterruptLostFocus
        );
        let full = session(Phase::Focus, u32::MAX, Some(10));
        assert!(full.tick(&instant(30), Some(10)).is_err());
    }
}
