//! Automatic failover.
//!
//! Detects that work has stopped so the app can hand a task to a web AI
//! without the developer asking. Two independent state machines:
//!
//! * **Local direction** — the developer's own terminal stopped touching
//!   the project (no fs events). `inactive → working → stalled →
//!   interrupted`. Any new activity vetoes the escalation.
//! * **Web direction** — the remote caller went quiet mid-work. There is no
//!   persistent socket left to observe, so this fires on inactivity alone.
//!   `working → stalled → interrupted`.
//!
//! Pure logic + unit tests; the app wiring (ticker thread, events, handoff
//! delivery) lives in `lib.rs`.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Agent {
    Local,
    Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum State {
    Inactive,
    Working,
    Stalled,
    Interrupted,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Inactive => "inactive",
            State::Working => "working",
            State::Stalled => "stalled",
            State::Interrupted => "interrupted",
        }
    }
}

/// Thresholds (durations) controlling the escalation ladder.
pub struct Thresholds {
    /// Local: idle after real work before we warn "stalled".
    pub local_stall_after: Duration,
    /// Local: idle after "stalled" before we confirm "interrupted".
    pub local_interrupt_after: Duration,
    /// Web: idle after web work before "stalled".
    pub web_stall_after: Duration,
    /// Web: idle after "stalled" before "interrupted". `check` still
    /// interrupts immediately on `ws_connected == false`, but the app wiring
    /// no longer passes `false` for the web direction, so inactivity is the
    /// only trigger in practice.
    pub web_interrupt_after: Duration,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            local_stall_after: Duration::from_secs(90),
            local_interrupt_after: Duration::from_secs(300),
            web_stall_after: Duration::from_secs(120),
            web_interrupt_after: Duration::from_secs(240),
        }
    }
}

#[derive(Debug)]
struct AgentState {
    state: State,
    last_activity: Instant,
    last_activity_kind: &'static str,
    fired: bool,
}

impl AgentState {
    fn new() -> Self {
        Self {
            state: State::Inactive,
            last_activity: Instant::now(),
            last_activity_kind: "none",
            fired: false,
        }
    }
}

/// What `check` decided this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Nothing changed.
    None,
    /// The agent went idle — warn (vetoable).
    Stalled,
    /// Confirmed interrupted — the failover trigger.
    Interrupted,
}

pub struct ActivityMonitor {
    pub thresholds: Thresholds,
    local: AgentState,
    web: AgentState,
}

impl Default for ActivityMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityMonitor {
    pub fn new() -> Self {
        Self::with_thresholds(Thresholds::default())
    }

    pub fn with_thresholds(thresholds: Thresholds) -> Self {
        Self {
            thresholds,
            local: AgentState::new(),
            web: AgentState::new(),
        }
    }

    pub fn state(&self, agent: Agent) -> State {
        match agent {
            Agent::Local => self.local.state,
            Agent::Web => self.web.state,
        }
    }

    /// Record that the given agent just did something. Any activity
    /// vetoes a pending stall/interrupt and re-arms a future failover.
    pub fn record_activity(&mut self, agent: Agent, kind: &'static str) {
        let s = self.state_mut(agent);
        s.last_activity = Instant::now();
        s.last_activity_kind = kind;
        if s.state != State::Working {
            s.state = State::Working;
            s.fired = false;
        }
    }

    /// Explicitly reset a state machine (used by "keep working" /
    /// dismissal in the UI). Re-arms failover without pretending the
    /// agent worked right now.
    pub fn reset(&mut self, agent: Agent) {
        let s = self.state_mut(agent);
        s.state = State::Working;
        s.last_activity = Instant::now();
        s.last_activity_kind = "reset";
        s.fired = false;
    }

    /// Advance the state machine for `agent`. `ws_connected` is the web
    /// direction's "remote still attached" flag; the app wiring now passes
    /// `true` unconditionally (there is no socket to drop), so only the idle
    /// windows drive an interruption.
    pub fn check(&mut self, agent: Agent, ws_connected: bool, now: Instant) -> Check {
        let s = match agent {
            Agent::Local => &mut self.local,
            Agent::Web => &mut self.web,
        };
        let idle = now.duration_since(s.last_activity);
        match s.state {
            State::Inactive => Check::None,
            State::Working => {
                let stall_after = match agent {
                    Agent::Local => self.thresholds.local_stall_after,
                    Agent::Web => self.thresholds.web_stall_after,
                };
                if agent == Agent::Web && !ws_connected && idle >= stall_after {
                    s.state = State::Interrupted;
                    s.fired = true;
                    Check::Interrupted
                } else if idle >= stall_after {
                    s.state = State::Stalled;
                    Check::Stalled
                } else {
                    Check::None
                }
            }
            State::Stalled => {
                let interrupt_after = match agent {
                    Agent::Local => self.thresholds.local_interrupt_after,
                    Agent::Web => self.thresholds.web_interrupt_after,
                };
                if !s.fired && (idle >= interrupt_after || (agent == Agent::Web && !ws_connected)) {
                    s.state = State::Interrupted;
                    s.fired = true;
                    Check::Interrupted
                } else {
                    Check::None
                }
            }
            State::Interrupted => Check::None,
        }
    }

    fn state_mut(&mut self, agent: Agent) -> &mut AgentState {
        match agent {
            Agent::Local => &mut self.local,
            Agent::Web => &mut self.web,
        }
    }
}

/// Idle time (ms) since the agent last did something, for telemetry.
pub fn idle_ms(monitor: &ActivityMonitor, agent: Agent) -> i64 {
    let s = match agent {
        Agent::Local => &monitor.local,
        Agent::Web => &monitor.web,
    };
    Instant::now().duration_since(s.last_activity).as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_thresholds() -> Thresholds {
        Thresholds {
            local_stall_after: Duration::from_secs(30),
            local_interrupt_after: Duration::from_secs(60),
            web_stall_after: Duration::from_secs(30),
            web_interrupt_after: Duration::from_secs(60),
        }
    }

    /// Move the agent's last-activity stamp `secs` into the past and
    /// return a fresh "now" for `check`.
    fn idle(m: &mut ActivityMonitor, agent: Agent, secs: u64) -> Instant {
        let s = match agent {
            Agent::Local => &mut m.local,
            Agent::Web => &mut m.web,
        };
        s.last_activity = Instant::now() - Duration::from_secs(secs);
        Instant::now()
    }

    #[test]
    fn local_escalates_and_activity_vetoes() {
        let mut m = ActivityMonitor::with_thresholds(fast_thresholds());
        assert_eq!(m.state(Agent::Local), State::Inactive);

        m.record_activity(Agent::Local, "fs");
        assert_eq!(m.state(Agent::Local), State::Working);
        let now = idle(&mut m, Agent::Local, 10);
        assert_eq!(m.check(Agent::Local, true, now), Check::None);

        let now = idle(&mut m, Agent::Local, 40);
        assert_eq!(m.check(Agent::Local, true, now), Check::Stalled);
        assert_eq!(m.state(Agent::Local), State::Stalled);

        // Veto: activity resumes.
        m.record_activity(Agent::Local, "fs");
        assert_eq!(m.state(Agent::Local), State::Working);

        // And re-escalates to interrupted once it stays idle past the cap.
        let now = idle(&mut m, Agent::Local, 70);
        assert_eq!(m.check(Agent::Local, true, now), Check::Stalled);
        let now = idle(&mut m, Agent::Local, 130);
        assert_eq!(m.check(Agent::Local, true, now), Check::Interrupted);
        assert_eq!(m.state(Agent::Local), State::Interrupted);
        // Fires only once until reset.
        let now = idle(&mut m, Agent::Local, 200);
        assert_eq!(m.check(Agent::Local, true, now), Check::None);
    }

    #[test]
    fn interrupted_reset_rearms() {
        let mut m = ActivityMonitor::with_thresholds(fast_thresholds());
        m.record_activity(Agent::Local, "fs");
        let now = idle(&mut m, Agent::Local, 70);
        assert_eq!(m.check(Agent::Local, true, now), Check::Stalled);
        let now = idle(&mut m, Agent::Local, 130);
        assert_eq!(m.check(Agent::Local, true, now), Check::Interrupted);
        m.reset(Agent::Local);
        assert_eq!(m.state(Agent::Local), State::Working);
        let now = idle(&mut m, Agent::Local, 70);
        assert_eq!(m.check(Agent::Local, true, now), Check::Stalled);
        let now = idle(&mut m, Agent::Local, 130);
        assert_eq!(m.check(Agent::Local, true, now), Check::Interrupted);
    }

    #[test]
    fn web_connected_idle_stalls_then_interrupts() {
        let mut m = ActivityMonitor::with_thresholds(fast_thresholds());
        m.record_activity(Agent::Web, "pair");
        // Remote still attached: a short idle stalls, it doesn't fire yet.
        let now = idle(&mut m, Agent::Web, 40);
        assert_eq!(m.check(Agent::Web, true, now), Check::Stalled);
        // But silence outliving the interrupt window is a dead remote caller,
        // even with the transport still attached.
        let now = idle(&mut m, Agent::Web, 90);
        assert_eq!(m.check(Agent::Web, true, now), Check::Interrupted);
        assert_eq!(m.state(Agent::Web), State::Interrupted);
    }

    #[test]
    fn web_ws_drop_triggers_interrupted() {
        let mut m = ActivityMonitor::with_thresholds(fast_thresholds());
        m.record_activity(Agent::Web, "tool");
        // The remote detaches while still in working → immediate interruption.
        let now = idle(&mut m, Agent::Web, 40);
        assert_eq!(m.check(Agent::Web, false, now), Check::Interrupted);
        assert_eq!(m.state(Agent::Web), State::Interrupted);
    }

    #[test]
    fn web_stalled_then_drop_or_timeout_interrupts() {
        let mut m = ActivityMonitor::with_thresholds(fast_thresholds());
        m.record_activity(Agent::Web, "tool");
        let now = idle(&mut m, Agent::Web, 40);
        assert_eq!(m.check(Agent::Web, true, now), Check::Stalled);
        // a detach while stalled is decisive...
        let now = idle(&mut m, Agent::Web, 45);
        assert_eq!(m.check(Agent::Web, false, now), Check::Interrupted);

        let mut m = ActivityMonitor::with_thresholds(fast_thresholds());
        m.record_activity(Agent::Web, "tool");
        let now = idle(&mut m, Agent::Web, 40);
        assert_eq!(m.check(Agent::Web, true, now), Check::Stalled);
        // ...or the stall simply outlives the interrupt window.
        let now = idle(&mut m, Agent::Web, 120);
        assert_eq!(m.check(Agent::Web, true, now), Check::Interrupted);
    }
}
