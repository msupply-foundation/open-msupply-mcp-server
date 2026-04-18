//! Formatting utilities -- port of src/types.ts.
//!
//! These produce the same human-readable text that the TypeScript server emits,
//! so tool output stays identical across the rewrite.

use serde_json::Value;

/// Pagination defaults: first=25, offset=0 -- matches `paginationVars` in types.ts.
pub fn pagination_vars(first: Option<u32>, offset: Option<u32>) -> (u32, u32) {
    (first.unwrap_or(25), offset.unwrap_or(0))
}

/// Format a single JSON object as key: value lines. Skips null and `__typename`.
/// Nested objects become JSON strings; arrays become `[N items]`.
pub fn format_record(record: &Value) -> String {
    let Some(map) = record.as_object() else {
        return record.to_string();
    };

    let mut lines = Vec::new();
    for (key, value) in map {
        if key == "__typename" || value.is_null() {
            continue;
        }
        let line = match value {
            Value::Array(arr) => format!("  {key}: [{} items]", arr.len()),
            Value::Object(_) => format!("  {key}: {value}"),
            Value::String(s) => format!("  {key}: {s}"),
            Value::Bool(b) => format!("  {key}: {b}"),
            Value::Number(n) => format!("  {key}: {n}"),
            Value::Null => unreachable!(),
        };
        lines.push(line);
    }
    lines.join("\n")
}

/// Format a paginated list response -- mirrors `formatListResult` in types.ts.
pub fn format_list_result(
    entity_name: &str,
    nodes: &[Value],
    total_count: u32,
    first: u32,
    offset: u32,
) -> String {
    if total_count == 0 {
        return format!("No {entity_name} found.");
    }

    let showing_end = (offset + first).min(total_count);
    let mut lines = vec![
        format!(
            "Found {total_count} {entity_name} (showing {start}-{end}):",
            start = offset + 1,
            end = showing_end
        ),
        String::new(),
    ];

    for node in nodes {
        lines.push(format_record(node));
        lines.push(String::new());
    }

    if offset + first < total_count {
        lines.push(format!(
            "... and {remaining} more. Use offset={next} to see the next page.",
            remaining = total_count - offset - first,
            next = offset + first
        ));
    }

    lines.join("\n")
}
