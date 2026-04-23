//! Shared test fixtures used across multiple modules.

use anyhow::{bail, Result};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::VecDeque;

use crate::runner::ApiClient;

/// A mock API client that returns pre-queued responses.
pub(crate) struct MockApiClient {
    pub(crate) responses: RefCell<VecDeque<Result<(Value, String)>>>,
}

impl MockApiClient {
    /// Create a mock client from a list of responses consumed in order.
    pub(crate) fn new(responses: Vec<Result<(Value, String)>>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
        }
    }

    /// Convenience: single response that ends the turn immediately.
    pub(crate) fn immediate_end() -> Self {
        Self::new(vec![Ok((
            serde_json::json!([{"type": "text", "text": "done"}]),
            "end_turn".into(),
        ))])
    }
}

impl ApiClient for MockApiClient {
    fn call(
        &self,
        _api_key: &str,
        _body: &Value,
        _stream: bool,
        _iteration: usize,
    ) -> Result<(Value, String)> {
        self.responses
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| bail!("no more mock responses"))
    }
}
