use crate::time::TimeStamp;
use serde::{Deserialize, Serialize};
use tehuti::third_party::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockEvent {
    Ping {
        id: u64,
        time: Duration,
    },
    Pong {
        id: u64,
        time: Duration,
        authority_time: Duration,
        authority_tick: TimeStamp,
    },
}

#[derive(Debug, Clone)]
pub struct AuthorityClock {
    pub tick: TimeStamp,
    timer: Instant,
}

impl Default for AuthorityClock {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl AuthorityClock {
    pub fn new(tick: TimeStamp) -> Self {
        Self {
            tick,
            timer: Instant::now(),
        }
    }

    #[must_use]
    pub fn pong(&self, event: ClockEvent) -> ClockEvent {
        match event {
            ClockEvent::Ping { id, time } => ClockEvent::Pong {
                id,
                time,
                authority_time: self.timer.elapsed(),
                authority_tick: self.tick,
            },
            ClockEvent::Pong { id, time, .. } => ClockEvent::Pong {
                id,
                time,
                authority_time: self.timer.elapsed(),
                authority_tick: self.tick,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimulationClock {
    pub tick: TimeStamp,
    timer: Instant,
    id_generator: u64,
    rtt: Duration,
    last_pong_time: Duration,
    last_authority_tick: TimeStamp,
    last_authority_time: Duration,
    authority_tick_rate: f64,
}

impl Default for SimulationClock {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl SimulationClock {
    pub fn new(tick: TimeStamp) -> Self {
        Self {
            tick,
            timer: Instant::now(),
            id_generator: 0,
            rtt: Duration::ZERO,
            last_pong_time: Duration::ZERO,
            last_authority_tick: tick,
            last_authority_time: Duration::ZERO,
            authority_tick_rate: 0.0,
        }
    }

    #[must_use]
    pub fn ping(&mut self) -> ClockEvent {
        self.id_generator += 1;
        let time = self.timer.elapsed();
        ClockEvent::Ping {
            id: self.id_generator,
            time,
        }
    }

    pub fn roundtrip(&mut self, event: ClockEvent) {
        if let ClockEvent::Pong {
            id,
            time,
            authority_time,
            authority_tick,
        } = event
            && id == self.id_generator
        {
            let now = self.timer.elapsed();
            let rtt_now = now - time;
            self.rtt = if self.rtt == Duration::ZERO {
                rtt_now
            } else {
                (self.rtt + rtt_now) / 2
            };
            let delta_ticks = authority_tick - self.last_authority_tick;
            let delta_time = authority_time - self.last_authority_time;
            let tick_rate = if delta_time > Duration::ZERO {
                delta_ticks as f64 / delta_time.as_secs_f64()
            } else {
                self.authority_tick_rate
            };
            self.authority_tick_rate = (self.authority_tick_rate + tick_rate) * 0.5;
            self.last_authority_tick = authority_tick;
            self.last_authority_time = authority_time;
            self.last_pong_time = now;
        }
    }

    pub fn rtt(&self) -> Duration {
        self.rtt
    }

    pub fn authority_tick_rate(&self) -> f64 {
        self.authority_tick_rate
    }

    pub fn estimate_current_authority_tick(&self) -> TimeStamp {
        let elapsed = self.timer.elapsed() - self.last_pong_time;
        let delta = (elapsed.as_secs_f64() * self.authority_tick_rate) as u64;
        TimeStamp::new(self.last_authority_tick.ticks() + delta)
    }

    pub fn estimate_target_tick(&self, lead_ticks: u64) -> TimeStamp {
        let estimated_authority_tick = self.estimate_current_authority_tick();
        TimeStamp::new(estimated_authority_tick.ticks() + lead_ticks)
    }
}

#[derive(Debug, Clone)]
pub enum Clock {
    Authority(AuthorityClock),
    Simulation(SimulationClock),
}

impl Clock {
    pub fn tick(&self) -> TimeStamp {
        match self {
            Clock::Authority(clock) => clock.tick,
            Clock::Simulation(clock) => clock.tick,
        }
    }

    pub fn set_tick(&mut self, tick: TimeStamp) {
        match self {
            Clock::Authority(clock) => clock.tick = tick,
            Clock::Simulation(clock) => clock.tick = tick,
        }
    }

    pub fn advance(&mut self, ticks: u64) {
        match self {
            Clock::Authority(clock) => clock.tick += ticks,
            Clock::Simulation(clock) => clock.tick += ticks,
        }
    }
}
