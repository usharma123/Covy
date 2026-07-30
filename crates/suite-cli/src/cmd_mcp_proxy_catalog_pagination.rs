use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::cmd_mcp::proxy_upstream::UpstreamClient;

const MAX_UPSTREAM_CATALOG_PAGES: usize = 128;
const MAX_UPSTREAM_CATALOG_ITEMS: usize = 16_384;
const MAX_UPSTREAM_CURSOR_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy)]
struct PaginationLimits {
    max_pages: usize,
    max_items: usize,
    max_cursor_bytes: usize,
}

impl Default for PaginationLimits {
    fn default() -> Self {
        Self {
            max_pages: MAX_UPSTREAM_CATALOG_PAGES,
            max_items: MAX_UPSTREAM_CATALOG_ITEMS,
            max_cursor_bytes: MAX_UPSTREAM_CURSOR_BYTES,
        }
    }
}

struct CatalogPagination {
    method: &'static str,
    item_key: &'static str,
    limits: PaginationLimits,
    pages: usize,
    items: Vec<Value>,
    next_cursor: Option<String>,
    seen_cursors: BTreeSet<String>,
}

impl CatalogPagination {
    fn new(method: &'static str, item_key: &'static str) -> Self {
        Self::with_limits(method, item_key, PaginationLimits::default())
    }

    fn with_limits(method: &'static str, item_key: &'static str, limits: PaginationLimits) -> Self {
        Self {
            method,
            item_key,
            limits,
            pages: 0,
            items: Vec::new(),
            next_cursor: None,
            seen_cursors: BTreeSet::new(),
        }
    }

    fn request(&self, upstream: &str) -> Value {
        let id = format!(
            "packet28-catalog-{upstream}-{}-{}",
            self.item_key,
            self.pages + 1
        );
        match &self.next_cursor {
            Some(cursor) => json!({
                "jsonrpc":"2.0",
                "id": id,
                "method": self.method,
                "params": {"cursor": cursor},
            }),
            None => json!({
                "jsonrpc":"2.0",
                "id": id,
                "method": self.method,
            }),
        }
    }

    fn accept_response(&mut self, upstream: &str, response: &Value) -> Result<bool> {
        self.pages = self.pages.checked_add(1).ok_or_else(|| {
            anyhow!(
                "upstream '{upstream}' {} page count overflowed",
                self.method
            )
        })?;
        if self.pages > self.limits.max_pages {
            return Err(anyhow!(
                "upstream '{upstream}' {} exceeds the page limit ({})",
                self.method,
                self.limits.max_pages
            ));
        }
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unspecified upstream error");
            return Err(anyhow!(
                "upstream '{upstream}' {} failed: {message}",
                self.method
            ));
        }
        let result = response
            .get("result")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                anyhow!(
                    "upstream '{upstream}' {} response is missing an object result",
                    self.method
                )
            })?;
        let page_items = result
            .get(self.item_key)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow!(
                    "upstream '{upstream}' {} result is missing array field '{}'",
                    self.method,
                    self.item_key
                )
            })?;
        let item_count = self
            .items
            .len()
            .checked_add(page_items.len())
            .ok_or_else(|| {
                anyhow!(
                    "upstream '{upstream}' {} item count overflowed",
                    self.method
                )
            })?;
        if item_count > self.limits.max_items {
            return Err(anyhow!(
                "upstream '{upstream}' {} exceeds the item limit ({})",
                self.method,
                self.limits.max_items
            ));
        }
        self.items.extend(page_items.iter().cloned());

        let Some(next_cursor) = result.get("nextCursor") else {
            self.next_cursor = None;
            return Ok(false);
        };
        let next_cursor = next_cursor.as_str().ok_or_else(|| {
            anyhow!(
                "upstream '{upstream}' {} returned a non-string nextCursor",
                self.method
            )
        })?;
        if next_cursor.len() > self.limits.max_cursor_bytes {
            return Err(anyhow!(
                "upstream '{upstream}' {} nextCursor exceeds the byte limit ({})",
                self.method,
                self.limits.max_cursor_bytes
            ));
        }
        if !self.seen_cursors.insert(next_cursor.to_string()) {
            return Err(anyhow!(
                "upstream '{upstream}' {} repeated cursor {next_cursor:?}",
                self.method
            ));
        }
        if self.pages == self.limits.max_pages {
            return Err(anyhow!(
                "upstream '{upstream}' {} exceeds the page limit ({})",
                self.method,
                self.limits.max_pages
            ));
        }
        self.next_cursor = Some(next_cursor.to_string());
        Ok(true)
    }

    fn into_items(self) -> Vec<Value> {
        self.items
    }
}

pub(crate) async fn collect_paginated_list(
    upstream: &Arc<UpstreamClient>,
    method: &'static str,
    item_key: &'static str,
) -> Result<Vec<Value>> {
    let mut pagination = CatalogPagination::new(method, item_key);
    loop {
        let response = upstream
            .send_request(&pagination.request(&upstream.name))
            .await
            .with_context(|| {
                format!(
                    "failed to fetch upstream '{}' {method} catalog",
                    upstream.name
                )
            })?;
        if !pagination.accept_response(&upstream.name, &response)? {
            return Ok(pagination.into_items());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_pages: usize, max_items: usize, max_cursor_bytes: usize) -> PaginationLimits {
        PaginationLimits {
            max_pages,
            max_items,
            max_cursor_bytes,
        }
    }

    #[test]
    fn pagination_accepts_empty_opaque_cursor_and_collects_every_page() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(3, 3, 8));
        pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[{"uri":"demo://one"}],"nextCursor":""}}),
            )
            .unwrap();

        pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[{"uri":"demo://two"}]}}),
            )
            .unwrap();

        assert_eq!(pagination.into_items().len(), 2);
    }

    #[test]
    fn pagination_rejects_repeated_cursor() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(3, 3, 8));
        pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[],"nextCursor":"same"}}),
            )
            .unwrap();

        let error = pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[],"nextCursor":"same"}}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("repeated cursor"));
    }

    #[test]
    fn pagination_rejects_more_pages_than_the_bound() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(1, 3, 8));

        let error = pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[],"nextCursor":"more"}}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("page limit (1)"));
    }

    #[test]
    fn pagination_rejects_item_resource_exhaustion() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(2, 1, 8));

        let error = pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[{"uri":"demo://one"},{"uri":"demo://two"}]}}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("item limit (1)"));
    }

    #[test]
    fn pagination_rejects_cursor_resource_exhaustion() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(2, 1, 2));

        let error = pagination
            .accept_response(
                "alpha",
                &json!({"result":{"resources":[],"nextCursor":"abc"}}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("byte limit (2)"));
    }

    #[test]
    fn pagination_rejects_malformed_page() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(2, 2, 8));

        let error = pagination
            .accept_response("alpha", &json!({"result":{"resources":null}}))
            .unwrap_err();

        assert!(error.to_string().contains("missing array"));
    }

    #[test]
    fn pagination_preserves_upstream_error_diagnostic() {
        let mut pagination =
            CatalogPagination::with_limits("resources/list", "resources", limits(2, 2, 8));

        let error = pagination
            .accept_response(
                "alpha",
                &json!({"error":{"code":-32000,"message":"catalog unavailable"}}),
            )
            .unwrap_err();

        assert!(error.to_string().contains("catalog unavailable"));
    }
}
