use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const HTTP_BUCKETS_MICROS: [u128; 7] = [
    100_000,
    250_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
    u128::MAX,
];
const HTTP_BUCKET_LABELS: [&str; 7] = ["0.1", "0.25", "0.5", "1", "2", "5", "+Inf"];

#[derive(Debug, Clone, Copy, Default)]
struct DurationAggregate {
    count: u64,
    sum_micros: u128,
    buckets: [u64; HTTP_BUCKETS_MICROS.len()],
}

impl DurationAggregate {
    fn record(&mut self, duration: Duration) {
        self.count = self.count.saturating_add(1);
        let micros = duration.as_micros();
        self.sum_micros = self.sum_micros.saturating_add(micros);
        for (index, upper_bound) in HTTP_BUCKETS_MICROS.iter().enumerate() {
            if micros <= *upper_bound {
                self.buckets[index] = self.buckets[index].saturating_add(1);
            }
        }
    }
}

#[derive(Debug)]
pub struct MetricsRegistry {
    started_at_seconds: u64,
    http_requests: Mutex<BTreeMap<(String, u16), u64>>,
    http_durations: Mutex<BTreeMap<String, DurationAggregate>>,
    auth_denied: AtomicU64,
    successful_mutations: AtomicU64,
    event_journal_failures: AtomicU64,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_at_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            http_requests: Mutex::new(BTreeMap::new()),
            http_durations: Mutex::new(BTreeMap::new()),
            auth_denied: AtomicU64::new(0),
            successful_mutations: AtomicU64::new(0),
            event_journal_failures: AtomicU64::new(0),
        }
    }

    pub fn record_http(&self, method: &str, status: u16, duration: Duration) {
        let method = normalized_method(method);
        *self
            .http_requests
            .lock()
            .expect("HTTP request metrics mutex poisoned")
            .entry((method.clone(), status))
            .or_default() += 1;
        self.http_durations
            .lock()
            .expect("HTTP duration metrics mutex poisoned")
            .entry(method)
            .or_default()
            .record(duration);
    }

    pub fn record_auth_denied(&self) {
        self.auth_denied.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_successful_mutation(&self) {
        self.successful_mutations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_event_journal_failure(&self) {
        self.event_journal_failures.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn render_prometheus(&self) -> String {
        let requests = self
            .http_requests
            .lock()
            .expect("HTTP request metrics mutex poisoned")
            .clone();
        let durations = self
            .http_durations
            .lock()
            .expect("HTTP duration metrics mutex poisoned")
            .clone();
        let mut output = String::from(
            "# HELP sessionweft_runtime_up Runtime process health.\n\
             # TYPE sessionweft_runtime_up gauge\n\
             sessionweft_runtime_up 1\n\
             # HELP sessionweft_process_start_time_seconds Unix process start time.\n\
             # TYPE sessionweft_process_start_time_seconds gauge\n",
        );
        output.push_str(&format!(
            "sessionweft_process_start_time_seconds {}\n",
            self.started_at_seconds
        ));
        output.push_str(
            "# HELP sessionweft_http_requests_total Completed HTTP requests.\n\
             # TYPE sessionweft_http_requests_total counter\n",
        );
        for ((method, status), count) in requests {
            output.push_str(&format!(
                "sessionweft_http_requests_total{{method=\"{method}\",status=\"{status}\"}} {count}\n"
            ));
        }
        output.push_str(
            "# HELP sessionweft_http_request_duration_seconds HTTP request duration by method.\n\
             # TYPE sessionweft_http_request_duration_seconds histogram\n",
        );
        for (method, aggregate) in durations {
            for (label, count) in HTTP_BUCKET_LABELS.iter().zip(aggregate.buckets) {
                output.push_str(&format!(
                    "sessionweft_http_request_duration_seconds_bucket{{method=\"{method}\",le=\"{label}\"}} {count}\n"
                ));
            }
            output.push_str(&format!(
                "sessionweft_http_request_duration_seconds_count{{method=\"{method}\"}} {}\n",
                aggregate.count
            ));
            output.push_str(&format!(
                "sessionweft_http_request_duration_seconds_sum{{method=\"{method}\"}} {:.6}\n",
                aggregate.sum_micros as f64 / 1_000_000.0
            ));
        }
        append_counter(
            &mut output,
            "sessionweft_auth_denied_total",
            "Denied bearer-token requests.",
            self.auth_denied.load(Ordering::Relaxed),
        );
        append_counter(
            &mut output,
            "sessionweft_successful_mutations_total",
            "Successful non-GET Runtime commands.",
            self.successful_mutations.load(Ordering::Relaxed),
        );
        append_counter(
            &mut output,
            "sessionweft_event_journal_failures_total",
            "Client event journal append failures.",
            self.event_journal_failures.load(Ordering::Relaxed),
        );
        output
    }
}

fn append_counter(output: &mut String, name: &str, help: &str, value: u64) {
    output.push_str(&format!("# HELP {name} {help}\n"));
    output.push_str(&format!("# TYPE {name} counter\n"));
    output.push_str(&format!("{name} {value}\n"));
}

fn normalized_method(method: &str) -> String {
    match method {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" | "HEAD" => method.to_owned(),
        _ => "OTHER".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_bounded_histogram_and_counters() {
        let registry = MetricsRegistry::new();
        registry.record_http("GET", 200, Duration::from_millis(25));
        registry.record_http("GET", 200, Duration::from_millis(750));
        registry.record_http("UNBOUNDED-CUSTOM", 503, Duration::from_millis(75));
        registry.record_auth_denied();
        registry.record_successful_mutation();
        registry.record_event_journal_failure();

        let metrics = registry.render_prometheus();
        assert!(
            metrics.contains("sessionweft_http_requests_total{method=\"GET\",status=\"200\"} 2")
        );
        assert!(
            metrics.contains("sessionweft_http_requests_total{method=\"OTHER\",status=\"503\"} 1")
        );
        assert!(metrics.contains(
            "sessionweft_http_request_duration_seconds_bucket{method=\"GET\",le=\"0.1\"} 1"
        ));
        assert!(metrics.contains(
            "sessionweft_http_request_duration_seconds_bucket{method=\"GET\",le=\"1\"} 2"
        ));
        assert!(metrics.contains("sessionweft_auth_denied_total 1"));
        assert!(metrics.contains("sessionweft_successful_mutations_total 1"));
        assert!(metrics.contains("sessionweft_event_journal_failures_total 1"));
    }
}
