use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};

const DOWNSTREAM_CATALOG_PAGE_ITEMS: usize = 256;
const DOWNSTREAM_CATALOG_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DOWNSTREAM_CATALOG_SNAPSHOTS: usize = 64;
const DOWNSTREAM_CATALOG_SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const DOWNSTREAM_CURSOR_PREFIX: &str = "packet28-resource-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResourceListKind {
    Resources,
    Templates,
}

impl ResourceListKind {
    fn cursor_label(self) -> &'static str {
        match self {
            Self::Resources => "resources",
            Self::Templates => "templates",
        }
    }

    fn result_key(self) -> &'static str {
        match self {
            Self::Resources => "resources",
            Self::Templates => "resourceTemplates",
        }
    }
}

struct DownstreamCatalogSnapshot {
    kind: ResourceListKind,
    items: Arc<[Value]>,
    expires_at: Instant,
}

pub(crate) struct DownstreamCatalogPager {
    snapshots: BTreeMap<u64, DownstreamCatalogSnapshot>,
    insertion_order: VecDeque<u64>,
    next_snapshot_id: u64,
    max_snapshots: usize,
    snapshot_ttl: Duration,
}

impl Default for DownstreamCatalogPager {
    fn default() -> Self {
        Self::with_limits(
            MAX_DOWNSTREAM_CATALOG_SNAPSHOTS,
            DOWNSTREAM_CATALOG_SNAPSHOT_TTL,
        )
    }
}

impl DownstreamCatalogPager {
    fn with_limits(max_snapshots: usize, snapshot_ttl: Duration) -> Self {
        Self {
            snapshots: BTreeMap::new(),
            insertion_order: VecDeque::new(),
            next_snapshot_id: 0,
            max_snapshots,
            snapshot_ttl,
        }
    }

    pub(crate) fn page(
        &mut self,
        kind: ResourceListKind,
        params: &Value,
        items: Vec<Value>,
    ) -> Result<Value> {
        self.page_at(kind, params, items, Instant::now())
    }

    fn page_at(
        &mut self,
        kind: ResourceListKind,
        params: &Value,
        items: Vec<Value>,
        now: Instant,
    ) -> Result<Value> {
        self.remove_expired(now);
        match catalog_cursor(params)? {
            None => {
                let items = Arc::<[Value]>::from(items);
                let end = downstream_page_end(&items, 0)?;
                if end == items.len() {
                    return render_catalog_page(kind, &items, 0, end, None);
                }
                let snapshot_id = self.insert_snapshot(kind, items.clone(), now)?;
                render_catalog_page(kind, &items, 0, end, Some(snapshot_id))
            }
            Some(cursor) => {
                let (snapshot_id, offset) = parse_downstream_cursor(kind, cursor)?;
                let snapshot = self.snapshots.get(&snapshot_id).ok_or_else(|| {
                    anyhow!("Packet28 resource catalog cursor is unknown or expired")
                })?;
                if snapshot.kind != kind {
                    return Err(anyhow!(
                        "Packet28 resource catalog cursor belongs to a different list method"
                    ));
                }
                if offset == 0 || offset >= snapshot.items.len() {
                    return Err(anyhow!(
                        "Packet28 resource catalog cursor offset is outside its snapshot"
                    ));
                }
                let end = downstream_page_end(&snapshot.items, offset)?;
                render_catalog_page(
                    kind,
                    &snapshot.items,
                    offset,
                    end,
                    (end < snapshot.items.len()).then_some(snapshot_id),
                )
            }
        }
    }

    fn insert_snapshot(
        &mut self,
        kind: ResourceListKind,
        items: Arc<[Value]>,
        now: Instant,
    ) -> Result<u64> {
        if self.max_snapshots == 0 {
            return Err(anyhow!(
                "Packet28 resource catalog snapshot capacity is disabled"
            ));
        }
        while self.snapshots.len() >= self.max_snapshots {
            let Some(oldest) = self.insertion_order.pop_front() else {
                return Err(anyhow!(
                    "Packet28 resource catalog snapshot accounting is inconsistent"
                ));
            };
            self.snapshots.remove(&oldest);
        }
        for _ in 0..=self.max_snapshots {
            self.next_snapshot_id = self.next_snapshot_id.wrapping_add(1).max(1);
            if self.snapshots.contains_key(&self.next_snapshot_id) {
                continue;
            }
            let snapshot_id = self.next_snapshot_id;
            self.snapshots.insert(
                snapshot_id,
                DownstreamCatalogSnapshot {
                    kind,
                    items,
                    expires_at: now + self.snapshot_ttl,
                },
            );
            self.insertion_order.push_back(snapshot_id);
            return Ok(snapshot_id);
        }
        Err(anyhow!(
            "Packet28 resource catalog snapshot id space is exhausted"
        ))
    }

    fn remove_expired(&mut self, now: Instant) {
        self.snapshots
            .retain(|_, snapshot| snapshot.expires_at > now);
        self.insertion_order
            .retain(|snapshot_id| self.snapshots.contains_key(snapshot_id));
    }
}

pub(crate) fn resource_catalog_continuation(params: &Value) -> Result<bool> {
    Ok(catalog_cursor(params)?.is_some())
}

fn catalog_cursor(params: &Value) -> Result<Option<&str>> {
    Ok(match params {
        Value::Null => None,
        Value::Object(object) => match object.get("cursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(cursor)) => Some(cursor.as_str()),
            Some(_) => return Err(anyhow!("resource catalog cursor must be a string")),
        },
        _ => return Err(anyhow!("resource catalog params must be an object")),
    })
}

fn downstream_page_end(items: &[Value], offset: usize) -> Result<usize> {
    let item_limit = offset
        .checked_add(DOWNSTREAM_CATALOG_PAGE_ITEMS)
        .map_or(items.len(), |end| end.min(items.len()));
    let mut bytes = 2_usize;
    let mut end = offset;
    while end < item_limit {
        let item_bytes = serde_json::to_vec(&items[end])?.len();
        let added = item_bytes
            .checked_add(usize::from(end > offset))
            .ok_or_else(|| anyhow!("resource catalog page byte count overflowed"))?;
        let next_bytes = bytes
            .checked_add(added)
            .ok_or_else(|| anyhow!("resource catalog page byte count overflowed"))?;
        if next_bytes > DOWNSTREAM_CATALOG_PAGE_BYTES {
            if end == offset {
                return Err(anyhow!(
                    "resource catalog item exceeds the page byte limit ({DOWNSTREAM_CATALOG_PAGE_BYTES})"
                ));
            }
            break;
        }
        bytes = next_bytes;
        end += 1;
    }
    Ok(end)
}

fn render_catalog_page(
    kind: ResourceListKind,
    items: &[Value],
    offset: usize,
    end: usize,
    next_snapshot_id: Option<u64>,
) -> Result<Value> {
    let mut result = Map::new();
    let page = items
        .get(offset..end)
        .ok_or_else(|| anyhow!("resource catalog page bounds are inconsistent"))?;
    result.insert(kind.result_key().to_string(), Value::Array(page.to_vec()));
    if let Some(snapshot_id) = next_snapshot_id {
        result.insert(
            "nextCursor".to_string(),
            Value::String(format!(
                "{DOWNSTREAM_CURSOR_PREFIX}:{}:{snapshot_id}:{end}",
                kind.cursor_label()
            )),
        );
    }
    Ok(Value::Object(result))
}

fn parse_downstream_cursor(kind: ResourceListKind, cursor: &str) -> Result<(u64, usize)> {
    let mut parts = cursor.split(':');
    let prefix = parts.next();
    let cursor_kind = parts.next();
    let snapshot_id = parts.next();
    let offset = parts.next();
    if prefix != Some(DOWNSTREAM_CURSOR_PREFIX)
        || cursor_kind != Some(kind.cursor_label())
        || snapshot_id.is_none()
        || offset.is_none()
        || parts.next().is_some()
    {
        return Err(anyhow!("invalid Packet28 resource catalog cursor"));
    }
    let snapshot_id = snapshot_id
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("invalid Packet28 resource catalog cursor snapshot"))?;
    let offset = offset
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| anyhow!("invalid Packet28 resource catalog cursor offset"))?;
    Ok((snapshot_id, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_items() -> Vec<Value> {
        (0..=DOWNSTREAM_CATALOG_PAGE_ITEMS)
            .map(|index| serde_json::json!({"uri": format!("demo://{index:03}")}))
            .collect()
    }

    #[test]
    fn cursor_pages_a_stable_snapshot() {
        let items = catalog_items();
        let mut pager = DownstreamCatalogPager::default();
        let first = pager
            .page(ResourceListKind::Resources, &Value::Null, items.clone())
            .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap();

        let second = pager
            .page(
                ResourceListKind::Resources,
                &serde_json::json!({"cursor": cursor}),
                items,
            )
            .unwrap();

        assert_eq!(second["resources"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn cursor_retains_original_snapshot_after_catalog_change() {
        let items = catalog_items();
        let mut pager = DownstreamCatalogPager::default();
        let first = pager
            .page(ResourceListKind::Resources, &Value::Null, items.clone())
            .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap();
        let mut changed = items;
        changed[0] = serde_json::json!({"uri": "demo://changed"});

        let second = pager
            .page(
                ResourceListKind::Resources,
                &serde_json::json!({"cursor": cursor}),
                changed,
            )
            .unwrap();

        assert_eq!(second["resources"][0]["uri"], "demo://256");
    }

    #[test]
    fn cursor_rejects_expired_snapshot() {
        let items = catalog_items();
        let now = Instant::now();
        let mut pager = DownstreamCatalogPager::with_limits(1, Duration::ZERO);
        let first = pager
            .page_at(
                ResourceListKind::Resources,
                &Value::Null,
                items.clone(),
                now,
            )
            .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap();

        let error = pager
            .page_at(
                ResourceListKind::Resources,
                &serde_json::json!({"cursor": cursor}),
                items,
                now,
            )
            .unwrap_err();

        assert!(error.to_string().contains("unknown or expired"));
    }

    #[test]
    fn cursor_is_bound_to_list_kind() {
        let items = catalog_items();
        let mut pager = DownstreamCatalogPager::default();
        let first = pager
            .page(ResourceListKind::Resources, &Value::Null, items.clone())
            .unwrap();
        let cursor = first["nextCursor"].as_str().unwrap();

        let error = pager
            .page(
                ResourceListKind::Templates,
                &serde_json::json!({"cursor": cursor}),
                items,
            )
            .unwrap_err();

        assert!(error.to_string().contains("invalid"));
    }

    #[test]
    fn page_rejects_an_item_larger_than_the_byte_budget() {
        let items = vec![serde_json::json!({
            "uri": "demo://oversized",
            "description": "x".repeat(DOWNSTREAM_CATALOG_PAGE_BYTES),
        })];
        let mut pager = DownstreamCatalogPager::default();

        let error = pager
            .page(ResourceListKind::Resources, &Value::Null, items)
            .unwrap_err();

        assert!(error.to_string().contains("page byte limit"));
    }
}
