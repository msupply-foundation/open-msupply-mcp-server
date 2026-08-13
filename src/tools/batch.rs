//! Bulk / batch tools — issue many inserts, updates and deletes in a single
//! GraphQL round-trip via the server's `batch*` mutations, instead of one MCP
//! tool call per line.
//!
//! - `insert_request_requisition_lines` is the ergonomic fix for the biggest
//!   bottleneck: adding many item lines to a request requisition (the per-line
//!   tool is one-at-a-time). It builds one `batchRequestRequisition` call.
//! - `batch_*` are passthrough wrappers over the raw batch mutations for power
//!   users / bulk seeding: pass the full batch `input` object (the operation
//!   lists) verbatim. Each reports a per-operation success/failure summary.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use serde_json::{Value, json};
use uuid::Uuid;

const INSERT_REQ_LINES_MUTATION: &str = r#"
  mutation batchInsertRequestLines($storeId: String!, $input: BatchRequestRequisitionInput!) {
    batchRequestRequisition(storeId: $storeId, input: $input) {
      insertRequestRequisitionLines {
        id
        response {
          __typename
          ... on RequisitionLineNode { id }
          ... on InsertRequestRequisitionLineError { error { __typename description } }
        }
      }
      updateRequestRequisitionLines {
        id
        response {
          __typename
          ... on RequisitionLineNode { id }
          ... on UpdateRequestRequisitionLineError { error { __typename description } }
        }
      }
    }
  }
"#;

const BATCH_REQUEST_REQUISITION_MUTATION: &str = r#"
  mutation batchRequestRequisition($storeId: String!, $input: BatchRequestRequisitionInput!) {
    batchRequestRequisition(storeId: $storeId, input: $input) {
      insertRequestRequisitions { id response { __typename } }
      insertRequestRequisitionLines { id response { __typename } }
      updateRequestRequisitions { id response { __typename } }
      updateRequestRequisitionLines { id response { __typename } }
      deleteRequestRequisitions { id response { __typename } }
      deleteRequestRequisitionLines { id response { __typename } }
    }
  }
"#;

const BATCH_INBOUND_SHIPMENT_MUTATION: &str = r#"
  mutation batchInboundShipment($storeId: String!, $input: BatchInboundShipmentInput!) {
    batchInboundShipment(storeId: $storeId, input: $input) {
      insertInboundShipments { id response { __typename } }
      insertInboundShipmentLines { id response { __typename } }
      updateInboundShipmentLines { id response { __typename } }
      deleteInboundShipmentLines { id response { __typename } }
      updateInboundShipments { id response { __typename } }
      deleteInboundShipments { id response { __typename } }
    }
  }
"#;

const BATCH_OUTBOUND_SHIPMENT_MUTATION: &str = r#"
  mutation batchOutboundShipment($storeId: String!, $input: BatchOutboundShipmentInput!) {
    batchOutboundShipment(storeId: $storeId, input: $input) {
      insertOutboundShipments { id response { __typename } }
      insertOutboundShipmentLines { id response { __typename } }
      updateOutboundShipmentLines { id response { __typename } }
      deleteOutboundShipmentLines { id response { __typename } }
      updateOutboundShipments { id response { __typename } }
      deleteOutboundShipments { id response { __typename } }
    }
  }
"#;

const BATCH_STOCKTAKE_MUTATION: &str = r#"
  mutation batchStocktake($storeId: String!, $input: BatchStocktakeInput!) {
    batchStocktake(storeId: $storeId, input: $input) {
      insertStocktakes { id response { __typename } }
      insertStocktakeLines { id response { __typename } }
      updateStocktakeLines { id response { __typename } }
      deleteStocktakeLines { id response { __typename } }
      updateStocktakes { id response { __typename } }
      deleteStocktakes { id response { __typename } }
    }
  }
"#;

/// Walk a batch response object: every field that is an array of `{id, response}`
/// counts as operations; an element whose `response.__typename` ends in "Error"
/// (or is a NodeError) is a failure. Returns (total, failure lines).
fn summarize_batch(resp: &Value) -> (usize, Vec<String>) {
    let mut total = 0usize;
    let mut failures = Vec::new();
    let Some(obj) = resp.as_object() else {
        return (0, failures);
    };
    for (op, arr) in obj {
        let Some(items) = arr.as_array() else { continue };
        for item in items {
            total += 1;
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let typename = item
                .pointer("/response/__typename")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if typename.ends_with("Error") {
                match item
                    .pointer("/response/error/description")
                    .and_then(|v| v.as_str())
                {
                    Some(desc) => failures.push(format!("  {op} {id}: {typename} — {desc}")),
                    None => failures.push(format!("  {op} {id}: {typename}")),
                }
            }
        }
    }
    (total, failures)
}

fn format_summary(label: &str, resp: &Value) -> String {
    let (total, failures) = summarize_batch(resp);
    let ok = total - failures.len();
    let mut out = vec![format!(
        "{label}: {total} operation(s), {ok} succeeded, {} failed.",
        failures.len()
    )];
    if !failures.is_empty() {
        out.push("Failures:".to_string());
        out.extend(failures);
    }
    out.join("\n")
}

/// Bulk-add item lines to an existing request requisition in one call.
pub async fn insert_request_requisition_lines(
    client: &OmSupplyClient,
    requisition_id: String,
    lines: Value,
    continue_on_error: Option<bool>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let arr = lines
        .as_array()
        .ok_or_else(|| AppError::Graphql("`lines` must be a JSON array".into()))?;
    if arr.is_empty() {
        return Err(AppError::Graphql("`lines` is empty".into()));
    }

    let mut insert_lines = Vec::with_capacity(arr.len());
    let mut update_lines = Vec::new();
    for (i, line) in arr.iter().enumerate() {
        let item_id = line
            .get("itemId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Graphql(format!("lines[{i}] missing itemId")))?;
        let line_id = line
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        insert_lines.push(json!({
            "id": line_id,
            "requisitionId": requisition_id,
            "itemId": item_id,
        }));

        let requested = line.get("requestedQuantity").filter(|v| !v.is_null());
        let comment = line.get("comment").and_then(|v| v.as_str());
        if requested.is_some() || comment.is_some() {
            let mut upd = json!({ "id": line_id });
            if let Some(q) = requested {
                upd["requestedQuantity"] = q.clone();
            }
            if let Some(c) = comment {
                upd["comment"] = json!(c);
            }
            update_lines.push(upd);
        }
    }

    let input = json!({
        "insertRequestRequisitionLines": insert_lines,
        "updateRequestRequisitionLines": update_lines,
        "continueOnError": continue_on_error.unwrap_or(false),
    });

    let data: Value = client
        .query(
            INSERT_REQ_LINES_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let resp = data
        .get("batchRequestRequisition")
        .ok_or_else(|| AppError::UnexpectedResponse("missing batchRequestRequisition".into()))?;
    Ok(format_summary(
        &format!("Added lines to requisition {requisition_id}"),
        resp,
    ))
}

/// Shared passthrough runner for the raw batch mutations.
async fn run_passthrough(
    client: &OmSupplyClient,
    mutation: &str,
    field: &str,
    mut input: Value,
    continue_on_error: Option<bool>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    if !input.is_object() {
        return Err(AppError::Graphql("`input` must be a JSON object".into()));
    }
    if let Some(c) = continue_on_error {
        input["continueOnError"] = json!(c);
    }

    let data: Value = client
        .query(mutation, json!({ "storeId": resolved_store_id, "input": input }))
        .await?;
    let resp = data
        .get(field)
        .ok_or_else(|| AppError::UnexpectedResponse(format!("missing {field}")))?;
    Ok(format_summary(field, resp))
}

pub async fn batch_request_requisition(
    client: &OmSupplyClient,
    input: Value,
    continue_on_error: Option<bool>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    run_passthrough(
        client,
        BATCH_REQUEST_REQUISITION_MUTATION,
        "batchRequestRequisition",
        input,
        continue_on_error,
        store_id,
    )
    .await
}

pub async fn batch_inbound_shipment(
    client: &OmSupplyClient,
    input: Value,
    continue_on_error: Option<bool>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    run_passthrough(
        client,
        BATCH_INBOUND_SHIPMENT_MUTATION,
        "batchInboundShipment",
        input,
        continue_on_error,
        store_id,
    )
    .await
}

pub async fn batch_outbound_shipment(
    client: &OmSupplyClient,
    input: Value,
    continue_on_error: Option<bool>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    run_passthrough(
        client,
        BATCH_OUTBOUND_SHIPMENT_MUTATION,
        "batchOutboundShipment",
        input,
        continue_on_error,
        store_id,
    )
    .await
}

pub async fn batch_stocktake(
    client: &OmSupplyClient,
    input: Value,
    continue_on_error: Option<bool>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    run_passthrough(
        client,
        BATCH_STOCKTAKE_MUTATION,
        "batchStocktake",
        input,
        continue_on_error,
        store_id,
    )
    .await
}
