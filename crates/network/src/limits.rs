//! Connection admission limits (P02-15).
//!
//! Two layers, both cheap and deterministic:
//! - a global semaphore caps concurrent connections at `max_players` plus a
//!   small headroom for status pings and login handshakes;
//! - a per-IP token bucket caps concurrent sockets per address and blunts
//!   reconnect storms while still allowing a normal burst of status pings
//!   (AGENTS.md section 10).
//!
//! Time is injected so tests are deterministic; production passes
//! [`std::time::Instant::now`]. The gate must be shared as `Arc<ConnectionGate>`
//! so guards can release their budget on drop.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Per-IP token bucket capacity (a burst of quick connections).
pub const PER_IP_BURST: f32 = 8.0;

/// Why a connection was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LimitError {
    /// The global connection budget is exhausted.
    #[error("the global connection budget is exhausted")]
    Global,
    /// The source address has too many concurrent connections.
    #[error("too many concurrent connections from this address")]
    PerIpConcurrent,
    /// The source address is reconnecting too quickly.
    #[error("reconnecting too quickly from this address")]
    PerIpRate,
}

/// Admission controller shared by all listeners.
#[derive(Debug)]
pub struct ConnectionGate {
    global: Arc<Semaphore>,
    state: Mutex<HashMap<IpAddr, PerIpState>>,
    max_per_ip: u32,
    refill_interval: Duration,
}

#[derive(Debug)]
struct PerIpState {
    concurrent: u32,
    tokens: f32,
    last_refill: Instant,
}

impl PerIpState {
    fn new(now: Instant) -> Self {
        Self {
            concurrent: 0,
            tokens: PER_IP_BURST,
            last_refill: now,
        }
    }

    fn refill(&mut self, now: Instant, interval: Duration) {
        let elapsed = now.saturating_duration_since(self.last_refill);
        if elapsed.is_zero() || interval.is_zero() {
            return;
        }
        let gained = elapsed.as_secs_f32() / interval.as_secs_f32();
        if gained > 0.0 {
            self.tokens = (self.tokens + gained).min(PER_IP_BURST);
            self.last_refill = now;
        }
    }
}

impl ConnectionGate {
    /// Create a gate.
    ///
    /// `global_capacity` is the total socket budget (players plus headroom),
    /// `max_per_ip` the per-address concurrent cap and `refill_interval` the
    /// time to regain one token after the [`PER_IP_BURST`] burst is spent.
    #[must_use]
    pub fn new(global_capacity: u32, max_per_ip: u32, refill_interval: Duration) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global_capacity.max(1) as usize)),
            state: Mutex::new(HashMap::new()),
            max_per_ip: max_per_ip.max(1),
            refill_interval,
        }
    }

    /// Lock the per-IP state, recovering from a poisoned mutex.
    ///
    /// A panic elsewhere while holding this lock must not turn every later
    /// admission into a process-wide panic (AGENTS.md section 9: never use panic
    /// as routine control flow). The guarded state is a plain counter map, so a
    /// recovered lock is still consistent enough to decide an admission; the
    /// worst case is a slightly stale token count for one address.
    fn lock_state(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<IpAddr, PerIpState>> {
        self.state.lock().unwrap_or_else(|poisoned| {
            tracing::error!("connection gate mutex was poisoned; continuing with recovered state");
            poisoned.into_inner()
        })
    }

    /// Try to admit a connection from `ip` at `now`.
    ///
    /// Returns a guard that releases the slot on drop.
    ///
    /// # Errors
    ///
    /// [`LimitError`] describing which budget refused the connection.
    pub fn try_acquire(
        self: &Arc<Self>,
        ip: IpAddr,
        now: Instant,
    ) -> Result<ConnectionGuard, LimitError> {
        let permit = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| LimitError::Global)?;
        let mut state = self.lock_state();
        let entry = state.entry(ip).or_insert_with(|| PerIpState::new(now));
        entry.refill(now, self.refill_interval);
        if entry.concurrent >= self.max_per_ip {
            return Err(LimitError::PerIpConcurrent);
        }
        if entry.tokens < 1.0 {
            return Err(LimitError::PerIpRate);
        }
        entry.tokens -= 1.0;
        entry.concurrent += 1;
        Ok(ConnectionGuard {
            gate: Arc::clone(self),
            ip,
            _permit: permit,
        })
    }

    /// Active connection count for `ip` (tests/diagnostics).
    #[must_use]
    pub fn concurrent_for(&self, ip: IpAddr) -> u32 {
        self.lock_state().get(&ip).map_or(0, |s| s.concurrent)
    }

    /// Number of tracked source addresses (diagnostics).
    #[must_use]
    pub fn tracked_ips(&self) -> usize {
        self.lock_state().len()
    }

    fn release(&self, ip: IpAddr, now: Instant) {
        let mut state = self.lock_state();
        if let Some(entry) = state.get_mut(&ip) {
            entry.concurrent = entry.concurrent.saturating_sub(1);
        }
        // Bound map growth under address-spray attacks: when the table is
        // large, forget idle addresses that have not connected recently.
        if state.len() > 4096 {
            state.retain(|_, entry| {
                entry.concurrent > 0
                    || now.saturating_duration_since(entry.last_refill) < Duration::from_secs(60)
            });
        }
    }
}

/// RAII admission slot. Dropping it frees the global permit and per-IP count.
#[derive(Debug)]
pub struct ConnectionGuard {
    gate: Arc<ConnectionGate>,
    ip: IpAddr,
    _permit: OwnedSemaphorePermit,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.gate.release(self.ip, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionGate, LimitError};
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, last))
    }

    #[test]
    fn global_budget_is_enforced_and_released_on_drop() {
        let gate = Arc::new(ConnectionGate::new(2, 8, Duration::ZERO));
        let now = Instant::now();
        let a = gate.try_acquire(ip(1), now).expect("first");
        let _b = gate.try_acquire(ip(2), now).expect("second");
        assert_eq!(gate.try_acquire(ip(3), now).err(), Some(LimitError::Global));
        drop(a);
        assert!(gate.try_acquire(ip(3), now).is_ok());
    }

    #[test]
    fn per_ip_concurrency_is_enforced() {
        let gate = Arc::new(ConnectionGate::new(16, 1, Duration::ZERO));
        let now = Instant::now();
        let guard = gate.try_acquire(ip(1), now).expect("first");
        assert_eq!(
            gate.try_acquire(ip(1), now).err(),
            Some(LimitError::PerIpConcurrent)
        );
        drop(guard);
        assert!(gate.try_acquire(ip(1), now).is_ok());
    }

    #[test]
    fn reconnect_burst_is_bounded_then_refills() {
        let gate = Arc::new(ConnectionGate::new(64, 64, Duration::from_millis(100)));
        let now = Instant::now();
        // The burst capacity is available immediately.
        for _ in 0..super::PER_IP_BURST as u32 {
            drop(gate.try_acquire(ip(1), now).expect("burst admission"));
        }
        assert_eq!(
            gate.try_acquire(ip(1), now).err(),
            Some(LimitError::PerIpRate)
        );
        // One refill interval regains one token.
        assert!(
            gate.try_acquire(ip(1), now + Duration::from_millis(120))
                .is_ok()
        );
    }

    #[test]
    fn concurrent_count_tracks_drops() {
        let gate = Arc::new(ConnectionGate::new(16, 4, Duration::ZERO));
        let now = Instant::now();
        let guard = gate.try_acquire(ip(1), now).expect("first");
        assert_eq!(gate.concurrent_for(ip(1)), 1);
        assert_eq!(gate.tracked_ips(), 1);
        drop(guard);
        assert_eq!(gate.concurrent_for(ip(1)), 0);
    }

    #[test]
    fn limit_errors_are_stable() {
        // P15-06: these variants previously had no Display at all; the messages
        // below are new but follow the variant docs word for word.
        let cases = [
            (
                LimitError::Global,
                "the global connection budget is exhausted",
            ),
            (
                LimitError::PerIpConcurrent,
                "too many concurrent connections from this address",
            ),
            (
                LimitError::PerIpRate,
                "reconnecting too quickly from this address",
            ),
        ];
        for error in cases {
            let _: &dyn std::error::Error = &error.0;
            assert_eq!(error.0.to_string(), error.1);
        }
    }
}
