use std::time::Instant;

use http_body::SizeHint;
use hyper::StatusCode;
use once_cell::sync::Lazy;
use prometheus::{
    exponential_buckets, opts, register_counter_vec, register_gauge_vec, register_histogram_vec,
    CounterVec, GaugeVec, HistogramVec,
};

use crate::runtime_config::RUNTIME_CONFIG;

const HTTP_LABEL_NAMES: [&str; 3] = ["app", "method", "status_code"];
const SOCKET_LABEL_NAMES: [&str; 1] = ["app"];

/// TODO: move this
enum Protocol {
    Http,
    Socket,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let as_str = match self {
            Protocol::Http => "http",
            Protocol::Socket => "socket",
        };

        write!(f, "{as_str}")
    }
}

/// Update HTTP metrics with a newly processed request.
#[inline(always)]
pub(crate) fn commit_http_metrics(
    labels: &[&str],
    start_time: &Instant,
    status_code: StatusCode,
    req_size: &SizeHint,
    res_size: &SizeHint,
) {
    let full_labels = vec![labels[0], labels[1], status_code.as_str()];
    HTTP_COUNTER.with_label_values(&full_labels).inc();

    HTTP_REQ_LAT_HISTOGRAM
        .with_label_values(&full_labels)
        .observe(start_time.elapsed().as_secs_f64());

    HTTP_REQ_SIZE_HISTOGRAM_LOW
        .with_label_values(&full_labels)
        .observe(req_size.lower() as f64);

    if let Some(size) = req_size.upper() {
        HTTP_REQ_SIZE_HISTOGRAM_HIGH
            .with_label_values(&full_labels)
            .observe(size as f64)
    }

    HTTP_RES_SIZE_HISTOGRAM_LOW
        .with_label_values(&full_labels)
        .observe(res_size.lower() as f64);

    if let Some(size) = req_size.upper() {
        HTTP_RES_SIZE_HISTOGRAM_HIGH
            .with_label_values(&full_labels)
            .observe(size as f64)
    }
}

/// A guard used to log metrics of a single socket connection, it ensures that the connection
/// counter will be incremented then decremented exactly once, even in case of a panic.
pub(crate) struct SocketMetricsGuard<'a> {
    app: &'a str,
}

impl<'a> SocketMetricsGuard<'a> {
    pub(crate) fn new(app: &'a str) -> Self {
        SOCKET_CONNECTED_HISTOGRAM.with_label_values(&[app]).inc();
        Self { app }
    }

    pub(crate) fn commit_message_sent(&self, size: usize) {
        SOCKET_MESSAGE_SENT_COUNTER
            .with_label_values(&[self.app])
            .inc();

        SOCKET_MESSAGE_SENT_SIZE_HISTOGRAM
            .with_label_values(&[self.app])
            .observe(size as f64)
    }

    pub(crate) fn commit_message_received(&self, size: usize) {
        SOCKET_MESSAGE_RECV_COUNTER
            .with_label_values(&[self.app])
            .inc();

        SOCKET_MESSAGE_RECV_SIZE_HISTOGRAM
            .with_label_values(&[self.app])
            .observe(size as f64)
    }
}

impl<'a> Drop for SocketMetricsGuard<'a> {
    fn drop(&mut self) {
        SOCKET_CONNECTED_HISTOGRAM
            .with_label_values(&[self.app])
            .dec();
    }
}

fn get_metric_name(name: &str, protocol: Protocol) -> String {
    format!(
        "gateway_{}_{protocol}_{name}",
        RUNTIME_CONFIG.metrics_prefix,
    )
}

static HTTP_COUNTER: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        opts!(
            get_metric_name("requests_total", Protocol::Http),
            "Number of HTTP requests made."
        ),
        &HTTP_LABEL_NAMES
    )
    .unwrap()
});

static HTTP_REQ_LAT_HISTOGRAM: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("request_duration_seconds", Protocol::Http),
        "The HTTP request latencies in seconds.",
        &HTTP_LABEL_NAMES
    )
    .unwrap()
});

static HTTP_REQ_SIZE_HISTOGRAM_LOW: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("request_size_low_bytes", Protocol::Http),
        "The HTTP request size in bytes (lower bound).",
        &HTTP_LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap()
});

static HTTP_REQ_SIZE_HISTOGRAM_HIGH: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("request_size_high_bytes", Protocol::Http),
        "The HTTP request size in bytes (upper bound).",
        &HTTP_LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap()
});

static HTTP_RES_SIZE_HISTOGRAM_LOW: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("response_size_low_bytes", Protocol::Http),
        "The HTTP response size in bytes (lower bound).",
        &HTTP_LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap()
});

static HTTP_RES_SIZE_HISTOGRAM_HIGH: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("response_size_high_bytes", Protocol::Http),
        "The HTTP response size in bytes (upper bound).",
        &HTTP_LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap()
});

static SOCKET_CONNECTED_HISTOGRAM: Lazy<GaugeVec> = Lazy::new(|| {
    register_gauge_vec!(
        get_metric_name("clients", Protocol::Socket),
        "Number simultaneously open sockets",
        &SOCKET_LABEL_NAMES,
    )
    .unwrap()
});

static SOCKET_MESSAGE_SENT_COUNTER: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        get_metric_name("message_sent", Protocol::Socket),
        "Total number of messages sent from server through sockets",
        &SOCKET_LABEL_NAMES,
    )
    .unwrap()
});

static SOCKET_MESSAGE_RECV_COUNTER: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        get_metric_name("message_received", Protocol::Socket),
        "Total number of messages received by server through sockets",
        &SOCKET_LABEL_NAMES,
    )
    .unwrap()
});

static SOCKET_MESSAGE_SENT_SIZE_HISTOGRAM: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("message_sent_size", Protocol::Socket),
        "Size of messages sent from server through sockets in bytes",
        &SOCKET_LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap()
});

static SOCKET_MESSAGE_RECV_SIZE_HISTOGRAM: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        get_metric_name("message_received", Protocol::Socket),
        "Size of messages received by server through sockets in bytes",
        &SOCKET_LABEL_NAMES,
        exponential_buckets(1.0, 2.0, 35).unwrap()
    )
    .unwrap()
});
