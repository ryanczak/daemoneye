//! Per-endpoint circuit breakers for AI requests, keyed by origin.
//!
//! One breaker per request origin (scheme, host, port): an endpoint that is
//! down never refuses a request to another endpoint. A breaker opens after
//! [`CB_FAILURE_THRESHOLD`] consecutive failures and stays open for
//! [`CB_COOLDOWN`] before half-open admits exactly one probe; the probe is a
//! permit the caller holds for the duration of one logical call (across its
//! internal retries) and resolves once. A dropped permit records nothing and
//! frees the probe slot.

use crate::util::UnpoisonExt;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Consecutive failures before a circuit trips open.
pub(crate) const CB_FAILURE_THRESHOLD: u32 = 5;
/// How long a circuit stays open before allowing a probe request.
pub(crate) const CB_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Default, Debug)]
pub(crate) struct BreakerState {
    pub(crate) consecutive_failures: u32,
    pub(crate) open_until: Option<Instant>,
    probe_in_flight: bool,
}

enum Admission {
    /// The caller may proceed; it must resolve the outcome once.
    Admit(bool),
    /// The circuit is open (or a probe is already in flight); refuse.
    Refuse,
}

impl BreakerState {
    fn admit(&mut self) -> Admission {
        match self.open_until {
            None => Admission::Admit(false),
            Some(t) if Instant::now() < t => Admission::Refuse,
            Some(_) => {
                if self.probe_in_flight {
                    Admission::Refuse
                } else {
                    self.probe_in_flight = true;
                    Admission::Admit(true)
                }
            }
        }
    }

    fn record_success(&mut self, is_probe: bool) {
        if is_probe {
            self.probe_in_flight = false;
        }
        self.consecutive_failures = 0;
        self.open_until = None;
    }

    fn record_failure(&mut self, is_probe: bool, origin: &str) {
        if is_probe {
            self.probe_in_flight = false;
        }
        self.consecutive_failures += 1;
        log::warn!(
            "AI backend request to {origin} failed ({}/{CB_FAILURE_THRESHOLD} consecutive failures).",
            self.consecutive_failures
        );
        if self.consecutive_failures >= CB_FAILURE_THRESHOLD {
            self.open_until = Some(Instant::now() + CB_COOLDOWN);
            log::warn!(
                "AI circuit breaker OPEN for {origin} after {failures} consecutive failures — \
                 cooling down for {}s before allowing a probe.",
                CB_COOLDOWN.as_secs(),
                failures = self.consecutive_failures
            );
        }
    }

    fn record_neutral(&mut self, is_probe: bool) {
        if is_probe {
            // The probe's 4xx means the endpoint answered, so half-open ends:
            // close the breaker and free the probe slot.
            self.probe_in_flight = false;
            self.open_until = None;
        }
        // A neutral outcome never moves the failure count (required behaviour 1).
    }

    fn release(&mut self, is_probe: bool) {
        if is_probe {
            self.probe_in_flight = false;
        }
    }
}

/// A held slot in one endpoint's breaker. Resolve it with exactly one of
/// [`CircuitPermit::record_success`], [`CircuitPermit::record_failure`] or
/// [`CircuitPermit::record_neutral`]; dropping it without resolving records
/// nothing and frees a half-open probe slot.
pub(crate) struct CircuitPermit {
    origin: String,
    is_probe: bool,
    resolved: bool,
}

impl CircuitPermit {
    fn resolve(&mut self, f: impl FnOnce(&mut BreakerState, bool)) {
        if self.resolved {
            return;
        }
        self.resolved = true;
        let mut registry = circuit_breakers().lock().unwrap_or_log();
        if let Some(state) = registry.get_mut(&self.origin) {
            f(state, self.is_probe);
        }
    }

    pub(crate) fn record_success(&mut self) {
        self.resolve(|s, probe| s.record_success(probe));
    }

    pub(crate) fn record_failure(&mut self) {
        let origin = self.origin.clone();
        self.resolve(|s, probe| s.record_failure(probe, &origin));
    }

    pub(crate) fn record_neutral(&mut self) {
        self.resolve(|s, probe| s.record_neutral(probe));
    }
}

impl Drop for CircuitPermit {
    fn drop(&mut self) {
        if !self.resolved {
            let mut registry = circuit_breakers().lock().unwrap_or_log();
            if let Some(state) = registry.get_mut(&self.origin) {
                state.release(self.is_probe);
            }
        }
    }
}

static CIRCUIT_BREAKERS: std::sync::OnceLock<Mutex<HashMap<String, BreakerState>>> =
    std::sync::OnceLock::new();

fn circuit_breakers() -> &'static Mutex<HashMap<String, BreakerState>> {
    CIRCUIT_BREAKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The origin (scheme, host, port) of a request URL. The breaker refuses and
/// logs by origin only — never the full URL, which for Gemini once carried
/// the API key (M36 phase-10, S4).
pub(crate) fn request_origin(req: &reqwest::Request) -> String {
    let url = req.url();
    match url.port_or_known_default() {
        Some(port) => format!(
            "{}://{}:{}",
            url.scheme(),
            url.host_str().unwrap_or(""),
            port
        ),
        None => format!("{}://{}", url.scheme(), url.host_str().unwrap_or("")),
    }
}

/// Try to acquire the breaker slot for `origin`. Returns `None` when the
/// circuit for that origin is open or a probe is already in flight.
pub(crate) fn acquire(origin: &str) -> Option<CircuitPermit> {
    let mut registry = circuit_breakers().lock().unwrap_or_log();
    let state = registry.entry(origin.to_string()).or_default();
    let is_probe = match state.admit() {
        Admission::Admit(is_probe) => is_probe,
        Admission::Refuse => {
            log::warn!(
                "AI circuit breaker is OPEN for {origin} — refusing the request; \
                 retry in ~{}s.",
                CB_COOLDOWN.as_secs()
            );
            return None;
        }
    };
    drop(registry);
    Some(CircuitPermit {
        origin: origin.to_string(),
        is_probe,
        resolved: false,
    })
}

#[cfg(test)]
pub(crate) fn breaker_for_test(origin: &str) -> BreakerState {
    circuit_breakers()
        .lock()
        .unwrap_or_log()
        .entry(origin.to_string())
        .or_default()
        .clone_shallow()
}

#[cfg(test)]
pub(crate) fn set_breaker_for_test(origin: &str, state: BreakerState) {
    circuit_breakers()
        .lock()
        .unwrap_or_log()
        .insert(origin.to_string(), state);
}

#[cfg(test)]
impl BreakerState {
    fn clone_shallow(&self) -> BreakerState {
        BreakerState {
            consecutive_failures: self.consecutive_failures,
            open_until: self.open_until,
            probe_in_flight: self.probe_in_flight,
        }
    }
}
