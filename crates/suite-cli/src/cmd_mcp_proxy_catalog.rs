use super::*;

use std::collections::BTreeSet;

use crate::cmd_mcp::proxy_catalog_pagination::collect_paginated_list;
use crate::cmd_mcp::proxy_resource::{ResourceRoute, ResourceRoutingTable};
use crate::cmd_mcp::proxy_resource_paging::{DownstreamCatalogPager, ResourceListKind};
use crate::cmd_mcp::proxy_upstream::UpstreamPool;

const MAX_COMBINED_CATALOG_ITEMS: usize = 65_536;
const MAX_RESOURCE_CATALOG_REFRESH_ATTEMPTS: usize = 3;

pub(crate) struct ProxyCatalog {
    refresh: tokio::sync::Mutex<()>,
    downstream_pages: Mutex<DownstreamCatalogPager>,
}

impl Default for ProxyCatalog {
    fn default() -> Self {
        Self {
            refresh: tokio::sync::Mutex::new(()),
            downstream_pages: Mutex::new(DownstreamCatalogPager::default()),
        }
    }
}

impl ProxyCatalog {
    pub(crate) fn page_resources(
        &self,
        kind: ResourceListKind,
        params: &Value,
        items: Vec<Value>,
    ) -> Result<Value> {
        self.downstream_pages
            .lock()
            .map_err(|_| anyhow!("failed to lock MCP resource catalog pages"))?
            .page(kind, params, items)
    }
}

#[derive(Clone)]
pub(crate) struct UpstreamResourceCatalog {
    pub(crate) resources: Vec<Value>,
    pub(crate) templates: Vec<Value>,
}

pub(crate) fn owner_for_tool(
    session: &Arc<Mutex<McpSessionState>>,
    tool_name: &str,
) -> Option<String> {
    session
        .lock()
        .ok()
        .and_then(|guard| guard.tool_owners.get(tool_name).cloned())
}

pub(crate) fn forward_name_for_tool(
    session: &Arc<Mutex<McpSessionState>>,
    tool_name: &str,
) -> Option<String> {
    session
        .lock()
        .ok()
        .and_then(|guard| guard.tool_forward_names.get(tool_name).cloned())
}

pub(crate) fn route_for_resource(
    session: &Arc<Mutex<McpSessionState>>,
    uri: &str,
) -> Result<ResourceRoute> {
    Ok(session
        .lock()
        .map_err(|_| anyhow!("failed to lock MCP session"))?
        .resource_routes
        .route(uri))
}

pub(crate) async fn ensure_upstream_tools_loaded(
    session: &Arc<Mutex<McpSessionState>>,
    upstreams: &Arc<UpstreamPool>,
    catalog: &ProxyCatalog,
) -> Result<Vec<Value>> {
    if let Ok(guard) = session.lock() {
        if guard.upstream_tools_loaded {
            return Ok(guard.upstream_tools_cache.clone());
        }
    }
    let _guard = catalog.refresh.lock().await;
    if let Ok(guard) = session.lock() {
        if guard.upstream_tools_loaded {
            return Ok(guard.upstream_tools_cache.clone());
        }
    }
    refresh_upstream_tools(session, upstreams).await
}

pub(crate) async fn ensure_upstream_resource_catalog_loaded(
    session: &Arc<Mutex<McpSessionState>>,
    upstreams: &Arc<UpstreamPool>,
    catalog: &ProxyCatalog,
) -> Result<UpstreamResourceCatalog> {
    if let Some(cached) = cached_resource_catalog(session)? {
        return Ok(cached);
    }
    let _guard = catalog.refresh.lock().await;
    for _ in 0..MAX_RESOURCE_CATALOG_REFRESH_ATTEMPTS {
        if let Some(cached) = cached_resource_catalog(session)? {
            return Ok(cached);
        }
        let epoch = session
            .lock()
            .map_err(|_| anyhow!("failed to lock MCP session"))?
            .resource_catalog_epoch;
        let discovered = load_upstream_resource_catalog(upstreams).await?;
        if let Some(published) = publish_resource_catalog(session, epoch, discovered)? {
            return Ok(published);
        }
    }
    Err(anyhow!(
        "upstream resource catalog changed during {} consecutive refresh attempts",
        MAX_RESOURCE_CATALOG_REFRESH_ATTEMPTS
    ))
}

pub(crate) fn invalidate_resource_catalog(session: &Arc<Mutex<McpSessionState>>) -> Result<()> {
    let mut guard = session
        .lock()
        .map_err(|_| anyhow!("failed to lock MCP session"))?;
    guard.resource_catalog_epoch = guard.resource_catalog_epoch.wrapping_add(1);
    guard.upstream_resource_catalog_loaded = false;
    guard.upstream_resources_cache.clear();
    guard.upstream_resource_templates_cache.clear();
    guard.resource_routes.clear();
    Ok(())
}

async fn refresh_upstream_tools(
    session: &Arc<Mutex<McpSessionState>>,
    upstreams: &Arc<UpstreamPool>,
) -> Result<Vec<Value>> {
    let native_tool_names = native_tool_names();
    let mut discovered = BTreeMap::<String, Vec<(String, Value)>>::new();
    let mut tool_owners = BTreeMap::new();
    for upstream in upstreams.values() {
        let response = upstream
            .send_request(&json!({
                "jsonrpc":"2.0",
                "id": format!("packet28-tools-refresh-{}", upstream.name),
                "method":"tools/list"
            }))
            .await?;
        if let Some(items) = response
            .get("result")
            .and_then(|value| value.get("tools"))
            .and_then(Value::as_array)
        {
            for item in items {
                if let Some(name) = item.get("name").and_then(Value::as_str) {
                    discovered
                        .entry(name.to_string())
                        .or_default()
                        .push((upstream.name.clone(), item.clone()));
                }
            }
        }
    }

    let mut tool_forward_names = BTreeMap::new();
    let mut rendered_tools = Vec::new();
    for (name, entries) in discovered {
        let needs_namespace = entries.len() > 1 || native_tool_names.contains_key(&name);
        for (owner, item) in entries {
            let alias = if needs_namespace {
                namespaced_tool_name(&owner, &name)
            } else {
                name.clone()
            };
            tool_owners.insert(alias.clone(), owner.clone());
            tool_forward_names.insert(alias.clone(), name.clone());
            rendered_tools.push(annotated_tool_item(item, &alias, &owner, needs_namespace));
        }
    }
    rendered_tools.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });

    if let Ok(mut guard) = session.lock() {
        guard.tool_owners = tool_owners;
        guard.tool_forward_names = tool_forward_names;
        guard.upstream_tools_cache = rendered_tools.clone();
        guard.upstream_tools_loaded = true;
    }
    Ok(rendered_tools)
}

fn cached_resource_catalog(
    session: &Arc<Mutex<McpSessionState>>,
) -> Result<Option<UpstreamResourceCatalog>> {
    let guard = session
        .lock()
        .map_err(|_| anyhow!("failed to lock MCP session"))?;
    Ok(guard
        .upstream_resource_catalog_loaded
        .then(|| UpstreamResourceCatalog {
            resources: guard.upstream_resources_cache.clone(),
            templates: guard.upstream_resource_templates_cache.clone(),
        }))
}

struct DiscoveredResourceCatalog {
    resources: Vec<Value>,
    templates: Vec<Value>,
    routes: ResourceRoutingTable,
}

fn publish_resource_catalog(
    session: &Arc<Mutex<McpSessionState>>,
    expected_epoch: u64,
    discovered: DiscoveredResourceCatalog,
) -> Result<Option<UpstreamResourceCatalog>> {
    let mut guard = session
        .lock()
        .map_err(|_| anyhow!("failed to lock MCP session"))?;
    if guard.resource_catalog_epoch != expected_epoch {
        return Ok(None);
    }
    guard.resource_routes = discovered.routes;
    guard.upstream_resources_cache = discovered.resources.clone();
    guard.upstream_resource_templates_cache = discovered.templates.clone();
    guard.upstream_resource_catalog_loaded = true;
    Ok(Some(UpstreamResourceCatalog {
        resources: discovered.resources,
        templates: discovered.templates,
    }))
}

async fn load_upstream_resource_catalog(
    upstreams: &Arc<UpstreamPool>,
) -> Result<DiscoveredResourceCatalog> {
    let mut resource_owners = BTreeMap::<String, BTreeSet<String>>::new();
    let mut template_owners = BTreeMap::<String, BTreeSet<String>>::new();
    let mut rendered_resources = Vec::<(String, String, Value)>::new();
    let mut rendered_templates = Vec::<(String, String, Value)>::new();
    for upstream in upstreams.values() {
        for item in collect_paginated_list(upstream, "resources/list", "resources").await? {
            let uri = item.get("uri").and_then(Value::as_str).ok_or_else(|| {
                anyhow!(
                    "upstream '{}' advertised a resource without a string uri",
                    upstream.name
                )
            })?;
            reject_reserved_resource_namespace(&upstream.name, "resource URI", uri)?;
            resource_owners
                .entry(uri.to_string())
                .or_default()
                .insert(upstream.name.clone());
            rendered_resources.push((uri.to_string(), upstream.name.clone(), item));
            if rendered_resources.len() > MAX_COMBINED_CATALOG_ITEMS {
                return Err(anyhow!(
                    "combined upstream resource catalog exceeds the item limit ({MAX_COMBINED_CATALOG_ITEMS})"
                ));
            }
        }
        for item in
            collect_paginated_list(upstream, "resources/templates/list", "resourceTemplates")
                .await?
        {
            let template = item
                .get("uriTemplate")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "upstream '{}' advertised a resource template without a string uriTemplate",
                        upstream.name
                    )
                })?;
            reject_reserved_resource_namespace(&upstream.name, "resource template", template)?;
            template_owners
                .entry(template.to_string())
                .or_default()
                .insert(upstream.name.clone());
            rendered_templates.push((template.to_string(), upstream.name.clone(), item));
            if rendered_templates.len() > MAX_COMBINED_CATALOG_ITEMS {
                return Err(anyhow!(
                    "combined upstream resource template catalog exceeds the item limit ({MAX_COMBINED_CATALOG_ITEMS})"
                ));
            }
        }
    }
    rendered_resources
        .sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    rendered_templates
        .sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    let mut routes = ResourceRoutingTable::default();
    routes.replace_exact(resource_owners);
    routes
        .replace_templates(template_owners)
        .context("upstream advertised an invalid or unsupported resource template")?;
    Ok(DiscoveredResourceCatalog {
        resources: rendered_resources
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
        templates: rendered_templates
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
        routes,
    })
}

fn reject_reserved_resource_namespace(owner: &str, kind: &str, value: &str) -> Result<()> {
    if value.starts_with("packet28://") {
        return Err(anyhow!(
            "upstream '{owner}' advertised {kind} {value:?} in Packet28's reserved namespace"
        ));
    }
    Ok(())
}

fn namespaced_tool_name(owner: &str, name: &str) -> String {
    let prefix = format!("{owner}.");
    if name.starts_with(&prefix) {
        name.to_string()
    } else {
        format!("{owner}.{name}")
    }
}

fn annotated_tool_item(mut item: Value, alias: &str, owner: &str, namespaced: bool) -> Value {
    if let Some(obj) = item.as_object_mut() {
        obj.insert("name".to_string(), Value::String(alias.to_string()));
        if namespaced {
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("Upstream MCP tool");
            obj.insert(
                "description".to_string(),
                Value::String(format!("{description} [via {owner}]")),
            );
        }
    }
    item
}

fn native_tool_names() -> BTreeMap<String, ()> {
    [
        "packet28.fetch_context",
        "packet28_fetch_context",
        "packet28.prepare_handoff",
        "packet28_prepare_handoff",
        "packet28.write_intention",
        "packet28_write_intention",
        "packet28.task_status",
        "packet28_task_status",
        "packet28.capabilities",
        "packet28_capabilities",
    ]
    .into_iter()
    .map(|name| (name.to_string(), ()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_catalog_builder_cannot_publish_after_epoch_invalidation() {
        let session = Arc::new(Mutex::new(McpSessionState::default()));
        let captured_epoch = session.lock().unwrap().resource_catalog_epoch;
        invalidate_resource_catalog(&session).unwrap();
        let discovered = DiscoveredResourceCatalog {
            resources: vec![json!({"uri":"demo://stale"})],
            templates: Vec::new(),
            routes: ResourceRoutingTable::default(),
        };

        let published = publish_resource_catalog(&session, captured_epoch, discovered).unwrap();

        assert!(published.is_none());
        assert!(!session.lock().unwrap().upstream_resource_catalog_loaded);
    }

    #[test]
    fn upstream_cannot_advertise_packet28_reserved_resource_namespace() {
        let error =
            reject_reserved_resource_namespace("spoof", "resource URI", "packet28://current/task")
                .unwrap_err();

        assert!(error.to_string().contains("reserved namespace"));
    }
}
