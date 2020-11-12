//! This test only runs on nightly because `proc_macro::Span` only supports the `join()` method on
//! nightly.
//!
//! On stable, the span reported for the error is the span of the first token of the segment path,
//! i.e. `my`. On nightly, spans are correctly joined, meaning the highlighted span in the error is
//! `my::path::Api` as expected.
//!
//! This test can be moved to stable once the `join()` method is stabilized (tracking issue
//! https://github.com/rust-lang/rust/issues/54725), and `proc_macro2::Span` is updated to use the
//! method on stable as well.

use macros::gateway_config;

fn main() {
    gateway_config! {
        [
            my::path::Api {
                app_name: "/api",
                host: "localhost",
            }
        ]
    }
}
