use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, Result};
use regex::Regex;

const SIMPLE_VALUE_PATTERN: &str = r"(?:[A-Za-z0-9._~-]|%[0-9A-Fa-f]{2})*";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResourceRoute {
    Missing,
    Unique(String),
    Ambiguous(Vec<String>),
}

#[derive(Clone, Debug)]
struct ResourceTemplateRoute {
    matcher: Regex,
    owners: BTreeSet<String>,
}

/// Routes resource URIs without choosing arbitrarily between upstream owners.
///
/// Template routing deliberately supports RFC 6570 Level 1 single-variable
/// simple expansions such as `{id}`. Expressions using operators, variable
/// lists, explode modifiers, or prefix modifiers are rejected while loading
/// the upstream catalog, so unsupported syntax can never broaden a route.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResourceRoutingTable {
    exact: BTreeMap<String, BTreeSet<String>>,
    templates: Vec<ResourceTemplateRoute>,
}

impl ResourceRoutingTable {
    pub(crate) fn clear(&mut self) {
        self.exact.clear();
        self.templates.clear();
    }

    pub(crate) fn replace_exact(&mut self, exact: BTreeMap<String, BTreeSet<String>>) {
        self.exact = exact;
    }

    pub(crate) fn replace_templates(
        &mut self,
        templates: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<()> {
        self.templates = templates
            .into_iter()
            .map(|(template, owners)| {
                Ok(ResourceTemplateRoute {
                    matcher: compile_level_one_template(&template)?,
                    owners,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(())
    }

    pub(crate) fn route(&self, uri: &str) -> ResourceRoute {
        let mut owners = self.exact.get(uri).cloned().unwrap_or_default();
        for template in &self.templates {
            if template.matcher.is_match(uri) {
                owners.extend(template.owners.iter().cloned());
            }
        }
        match owners.len() {
            0 => ResourceRoute::Missing,
            1 => owners
                .into_iter()
                .next()
                .map_or(ResourceRoute::Missing, ResourceRoute::Unique),
            _ => ResourceRoute::Ambiguous(owners.into_iter().collect()),
        }
    }
}

fn compile_level_one_template(template: &str) -> Result<Regex> {
    let mut pattern = String::from("^");
    let mut remainder = template;
    while let Some(open) = remainder.find('{') {
        let literal = &remainder[..open];
        if literal.contains('}') {
            return Err(anyhow!("resource template has an unmatched closing brace"));
        }
        pattern.push_str(&regex::escape(literal));

        let expression_tail = &remainder[open + 1..];
        let close = expression_tail
            .find('}')
            .ok_or_else(|| anyhow!("resource template has an unclosed expression"))?;
        let expression = &expression_tail[..close];
        if expression.contains('{') {
            return Err(anyhow!("resource template has a nested expression"));
        }
        validate_level_one_variable(expression)?;
        pattern.push_str(SIMPLE_VALUE_PATTERN);
        remainder = &expression_tail[close + 1..];
    }
    if remainder.contains('}') {
        return Err(anyhow!("resource template has an unmatched closing brace"));
    }
    pattern.push_str(&regex::escape(remainder));
    pattern.push('$');
    Regex::new(&pattern).map_err(Into::into)
}

fn validate_level_one_variable(variable: &str) -> Result<()> {
    if variable.is_empty() {
        return Err(anyhow!("resource template has an empty expression"));
    }
    if variable
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'+' | b'#' | b'.' | b'/' | b';' | b'?' | b'&'))
        || variable
            .bytes()
            .any(|byte| matches!(byte, b',' | b'*' | b':'))
    {
        return Err(anyhow!(
            "unsupported RFC 6570 expression '{{{variable}}}'; only Level 1 single-variable simple expansion is supported"
        ));
    }
    if !valid_variable_name(variable.as_bytes()) {
        return Err(anyhow!(
            "resource template expression '{{{variable}}}' has an invalid variable name"
        ));
    }
    Ok(())
}

fn valid_variable_name(bytes: &[u8]) -> bool {
    let mut index = 0;
    let mut segment_len = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'.' if segment_len > 0 => {
                segment_len = 0;
                index += 1;
            }
            byte if byte.is_ascii_alphanumeric() || byte == b'_' => {
                segment_len += 1;
                index += 1;
            }
            b'%' if index + 2 < bytes.len()
                && bytes[index + 1].is_ascii_hexdigit()
                && bytes[index + 2].is_ascii_hexdigit() =>
            {
                segment_len += 1;
                index += 3;
            }
            _ => return false,
        }
    }
    segment_len > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owners(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn level_one_template_matches_unreserved_and_percent_encoded_values() {
        let matcher = compile_level_one_template("demo://items/{item_id}/detail.json").unwrap();

        assert!(matcher.is_match("demo://items/a-._~9%2F/detail.json"));
    }

    #[test]
    fn level_one_template_does_not_match_reserved_path_expansion() {
        let matcher = compile_level_one_template("demo://items/{item_id}/detail").unwrap();

        assert!(!matcher.is_match("demo://items/a/b/detail"));
    }

    #[test]
    fn level_one_template_rejects_unsupported_rfc6570_operators_and_modifiers() {
        for template in [
            "demo://items/{+path}",
            "demo://items/{id,slug}",
            "demo://items/{id*}",
            "demo://items/{id:3}",
            "demo://items/{?id}",
        ] {
            assert!(
                compile_level_one_template(template).is_err(),
                "unsupported template was accepted: {template}"
            );
        }
    }

    #[test]
    fn level_one_template_rejects_malformed_expressions_and_names() {
        for template in [
            "demo://items/{",
            "demo://items/{}",
            "demo://items/{outer{inner}}",
            "demo://items/id}",
            "demo://items/{bad-name}",
            "demo://items/{a..b}",
            "demo://items/{a.}",
            "demo://items/{%GG}",
        ] {
            assert!(
                compile_level_one_template(template).is_err(),
                "malformed template was accepted: {template}"
            );
        }
    }

    #[test]
    fn route_unites_exact_and_all_matching_template_owners() {
        let mut routes = ResourceRoutingTable::default();
        routes.replace_exact(BTreeMap::from([(
            "demo://items/42".to_string(),
            owners(&["exact"]),
        )]));
        routes
            .replace_templates(BTreeMap::from([
                ("demo://items/{id}".to_string(), owners(&["template"])),
                ("demo://{kind}/42".to_string(), owners(&["other"])),
            ]))
            .unwrap();

        assert_eq!(
            routes.route("demo://items/42"),
            ResourceRoute::Ambiguous(vec![
                "exact".to_string(),
                "other".to_string(),
                "template".to_string(),
            ])
        );
    }

    #[test]
    fn route_deduplicates_one_owner_advertising_exact_and_template_forms() {
        let mut routes = ResourceRoutingTable::default();
        routes.replace_exact(BTreeMap::from([(
            "demo://items/42".to_string(),
            owners(&["alpha"]),
        )]));
        routes
            .replace_templates(BTreeMap::from([(
                "demo://items/{id}".to_string(),
                owners(&["alpha"]),
            )]))
            .unwrap();

        assert_eq!(
            routes.route("demo://items/42"),
            ResourceRoute::Unique("alpha".to_string())
        );
    }
}
