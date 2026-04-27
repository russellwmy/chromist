//! Accessibility tree snapshot via `Accessibility.getFullAXTree`.

use std::collections::HashMap;

use crate::cdp::browser_protocol::accessibility as cdp_ax;

/// One node in the page's accessibility tree.
///
/// Returned by [`Page::accessibility_snapshot`](crate::Page::accessibility_snapshot).
#[derive(Debug, Clone)]
pub struct AXNode {
    /// ARIA role (e.g. `"button"`, `"link"`, `"heading"`).
    pub role: Option<String>,
    /// Accessible name from `aria-label`, label text, or alt text.
    pub name: Option<String>,
    /// Accessible description (`aria-describedby` text or similar).
    pub description: Option<String>,
    /// Current value (for inputs and range widgets).
    pub value: Option<String>,
    /// Whether this node is hidden from assistive technology.
    pub ignored: bool,
    /// Child nodes in document order.
    pub children: Vec<AXNode>,
}

pub(crate) fn ax_value_string(v: &cdp_ax::AxValue) -> Option<String> {
    v.value.as_ref().map(|j| match j {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

pub(crate) fn convert(
    node: &cdp_ax::AxNode,
    nodes_by_id: &HashMap<String, &cdp_ax::AxNode>,
) -> AXNode {
    let children = node
        .child_ids
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|cid| nodes_by_id.get(cid.0.as_str()))
        .filter(|c| !c.ignored)
        .map(|c| convert(c, nodes_by_id))
        .collect();

    AXNode {
        role: node.role.as_ref().and_then(ax_value_string),
        name: node.name.as_ref().and_then(ax_value_string),
        description: node.description.as_ref().and_then(ax_value_string),
        value: node.value.as_ref().and_then(ax_value_string),
        ignored: node.ignored,
        children,
    }
}

pub(crate) fn build_tree(nodes: &[cdp_ax::AxNode]) -> Option<AXNode> {
    let nodes_by_id: HashMap<String, &cdp_ax::AxNode> =
        nodes.iter().map(|n| (n.node_id.0.clone(), n)).collect();

    let root = nodes.iter().find(|n| {
        n.parent_id.as_ref().is_none_or(|pid| !nodes_by_id.contains_key(pid.0.as_str()))
    })?;

    Some(convert(root, &nodes_by_id))
}
