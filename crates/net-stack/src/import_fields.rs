//! Lossless import adapters shared by supported formats. No protocol capability fallback.
use crate::clash_parser::ClashParseResult;
use polaris_config_engine::user_config::server_config::{
    lands_in_endpoints, Protocol, ServerConfig,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn string_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value? {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(values) => values
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

pub(crate) fn http_headers(value: Option<&Value>) -> Option<BTreeMap<String, Vec<String>>> {
    value?
        .as_object()?
        .iter()
        .map(|(key, value)| Some((key.clone(), string_list(Some(value))?)))
        .collect()
}

pub(crate) fn ws_headers(value: Option<&Value>) -> Option<BTreeMap<String, String>> {
    http_headers(value)?
        .into_iter()
        .map(|(key, values)| {
            if values.len() == 1 {
                Some((key, values.into_iter().next()?))
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn headers_supported(value: Option<&Value>, multiple: bool) -> bool {
    value.is_none()
        || if multiple {
            http_headers(value).is_some()
        } else {
            ws_headers(value).is_some()
        }
}

/// An explicit dependency cannot become an absent dependency, even for malformed input.
pub(crate) fn valid_detour(value: Option<&Value>) -> bool {
    value.is_none()
        || value
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
}

#[derive(Default)]
pub(crate) struct ImportGraph {
    source_counts: BTreeMap<String, usize>,
    accepted_tags: BTreeMap<String, String>,
}
impl ImportGraph {
    pub(crate) fn observe(&mut self, tag: Option<&Value>) {
        if let Some(tag) = tag.and_then(Value::as_str) {
            *self.source_counts.entry(tag.to_owned()).or_default() += 1;
        }
    }
    pub(crate) fn register(
        &mut self,
        server: &mut ServerConfig,
        tag: Option<&Value>,
        detour: Option<&Value>,
    ) {
        if let Some(tag) = tag.and_then(Value::as_str) {
            self.accepted_tags.insert(server.id.clone(), tag.to_owned());
        }
        // This temporary external tag never escapes finish().
        server.detour = detour.and_then(Value::as_str).map(str::to_owned);
    }
    pub(crate) fn finish(self, result: ClashParseResult) -> ClashParseResult {
        // TLS rejection precedes graph resolution, so dependencies on rejected pins also fail closed.
        let mut result = result.finish();
        let by_tag: BTreeMap<&str, usize> = result
            .servers
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let tag = self.accepted_tags.get(&s.id)?;
                (self.source_counts.get(tag) == Some(&1)).then_some((tag.as_str(), i))
            })
            .collect();
        let edges: Vec<Result<Option<usize>, ()>> = result
            .servers
            .iter()
            .map(|s| {
                let Some(tag) = s.detour.as_deref() else {
                    return Ok(None);
                };
                let target = *by_tag.get(tag).ok_or(())?;
                let protocol = result.servers[target].protocol;
                if lands_in_endpoints(protocol) || protocol == Protocol::Custom {
                    return Err(());
                }
                Ok(Some(target))
            })
            .collect();
        let mut valid = vec![None; edges.len()];
        for start in 0..edges.len() {
            if valid[start].is_some() {
                continue;
            }
            let mut path = BTreeSet::new();
            let mut current = start;
            let verdict = loop {
                if let Some(verdict) = valid[current] {
                    break verdict;
                }
                if !path.insert(current) {
                    break false;
                }
                match edges[current] {
                    Ok(Some(next)) => current = next,
                    Ok(None) => break true,
                    Err(()) => break false,
                }
            };
            for index in path {
                valid[index] = Some(verdict);
            }
        }
        let ids: Vec<String> = result.servers.iter().map(|s| s.id.clone()).collect();
        let before = result.servers.len();
        result.servers = result
            .servers
            .into_iter()
            .enumerate()
            .filter_map(|(i, mut s)| {
                if valid[i] != Some(true) {
                    return None;
                }
                s.detour = edges[i].ok().flatten().map(|target| ids[target].clone());
                Some(s)
            })
            .collect();
        let rejected = before - result.servers.len();
        if rejected > 0 {
            result.failed += rejected;
            result.warnings.push(format!("{rejected} nodes rejected: unresolved, ambiguous, cyclic or unsupported import detour (IMPORT_DETOUR_INVALID)"));
        }
        result
    }
}
