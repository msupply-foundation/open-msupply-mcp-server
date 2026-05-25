//! R&R Form tools — periodic report-and-requisition forms.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const RNR_FORMS_QUERY: &str = r#"
  query rAndRForms(
    $storeId: String!
    $first: Int
    $offset: Int
    $key: RnRFormSortFieldInput!
    $desc: Boolean
    $filter: RnRFormFilterInput
  ) {
    rAndRForms(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
    ) {
      ... on RnRFormConnector {
        __typename
        totalCount
        nodes {
          id status programName supplierName createdDatetime
          theirReference comment periodLength
          period { id name startDate endDate }
        }
      }
    }
  }
"#;

const RNR_FORM_DETAIL_QUERY: &str = r#"
  query rAndRForm($storeId: String!, $rnrFormId: String!) {
    rAndRForm(storeId: $storeId, rnrFormId: $rnrFormId) {
      ... on RnRFormNode {
        __typename
        id status programId programName supplierId supplierName
        createdDatetime theirReference comment periodLength
        period { id name startDate endDate }
        lines {
          id itemId averageMonthlyConsumption initialBalance
          quantityReceived quantityConsumed adjustedQuantityConsumed
          losses adjustments stockOutDuration finalBalance
          minimumQuantity maximumQuantity expiryDate
          calculatedRequestedQuantity enteredRequestedQuantity
          lowStock comment confirmed approvedQuantity
          item { id code name unitName }
        }
      }
      ... on NodeError {
        __typename
        error { description }
      }
    }
  }
"#;

const INSERT_RNR_FORM_MUTATION: &str = r#"
  mutation insertRnrForm($storeId: String!, $input: InsertRnRFormInput!) {
    insertRnrForm(storeId: $storeId, input: $input) {
      __typename
      ... on RnRFormNode {
        id status programId programName supplierId supplierName
        createdDatetime theirReference comment periodLength
        period { id name startDate endDate }
        lines {
          id itemId averageMonthlyConsumption initialBalance
          quantityReceived quantityConsumed adjustedQuantityConsumed
          losses adjustments stockOutDuration finalBalance
          minimumQuantity maximumQuantity expiryDate
          calculatedRequestedQuantity enteredRequestedQuantity
          lowStock comment confirmed approvedQuantity
          item { id code name unitName }
        }
      }
    }
  }
"#;

const UPDATE_RNR_FORM_MUTATION: &str = r#"
  mutation updateRnrForm($storeId: String!, $input: UpdateRnRFormInput!) {
    updateRnrForm(storeId: $storeId, input: $input) {
      __typename
      ... on RnRFormNode {
        id status programId programName supplierId supplierName
        createdDatetime theirReference comment periodLength
        period { id name startDate endDate }
        lines {
          id itemId averageMonthlyConsumption initialBalance
          quantityReceived quantityConsumed adjustedQuantityConsumed
          losses adjustments stockOutDuration finalBalance
          minimumQuantity maximumQuantity expiryDate
          calculatedRequestedQuantity enteredRequestedQuantity
          lowStock comment confirmed approvedQuantity
          item { id code name unitName }
        }
      }
    }
  }
"#;

const FINALISE_RNR_FORM_MUTATION: &str = r#"
  mutation finaliseRnrForm($storeId: String!, $input: FinaliseRnRFormInput!) {
    finaliseRnrForm(storeId: $storeId, input: $input) {
      __typename
      ... on RnRFormNode {
        id status programName supplierName
      }
    }
  }
"#;

const DELETE_RNR_FORM_MUTATION: &str = r#"
  mutation deleteRnrForm($storeId: String!, $input: DeleteRnRFormInput!) {
    deleteRnrForm(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
    }
  }
"#;

#[derive(Deserialize)]
struct RnrFormsResp {
    #[serde(rename = "rAndRForms")]
    forms: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct RnrFormDetailResp {
    #[serde(rename = "rAndRForm")]
    form: Value,
}

pub async fn list_rnr_forms(
    client: &OmSupplyClient,
    program_id: Option<String>,
    period_schedule_id: Option<String>,
    sort_by: Option<String>,
    desc: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(p) = program_id {
        filter.insert("programId".into(), json!({ "equalTo": p }));
    }
    if let Some(s) = period_schedule_id {
        filter.insert("periodScheduleId".into(), json!({ "equalTo": s }));
    }

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let key = sort_by.unwrap_or_else(|| "createdDatetime".into());

    let data: RnrFormsResp = client
        .query(
            RNR_FORMS_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "key": key,
                "desc": desc.unwrap_or(true),
                "filter": filter_value,
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "R&R forms",
        &data.forms.nodes,
        data.forms.total_count,
        first,
        offset,
    ))
}

pub async fn get_rnr_form(
    client: &OmSupplyClient,
    rnr_form_id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: RnrFormDetailResp = client
        .query(
            RNR_FORM_DETAIL_QUERY,
            json!({ "rnrFormId": rnr_form_id, "storeId": resolved_store_id }),
        )
        .await?;

    let typename = data
        .form
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if typename != "RnRFormNode" {
        let desc = data
            .form
            .pointer("/error/description")
            .and_then(|v| v.as_str())
            .unwrap_or("R&R form not found");
        return Err(AppError::Graphql(desc.to_string()));
    }

    Ok(format_rnr_form_with_lines(&data.form, "R&R form details"))
}

/// Render an R&R form with each line broken out as its own block, so line UUIDs
/// (and all other fields needed to construct an UpdateRnRFormLineInput) are visible.
fn format_rnr_form_with_lines(form: &Value, header: &str) -> String {
    let mut out = vec![format!("{header}:")];

    // Header fields, omitting the lines array (we'll render it below).
    if let Some(map) = form.as_object() {
        for (key, value) in map {
            if key == "__typename" || key == "lines" || value.is_null() {
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
            out.push(line);
        }
    }

    if let Some(lines) = form.get("lines").and_then(|v| v.as_array()) {
        out.push(String::new());
        out.push(format!("Lines ({} total):", lines.len()));
        for (i, line) in lines.iter().enumerate() {
            out.push(String::new());
            out.push(format!("--- Line {} ---", i + 1));
            out.push(format_record(line));
        }
        out.push(String::new());
        out.push(
            "To update lines, pass an array of UpdateRnRFormLineInput to update_rnr_form. \
             Each entry must include the line `id` shown above plus the required fields \
             (stockOutDuration, adjustedQuantityConsumed, averageMonthlyConsumption, \
             initialBalance, finalBalance, minimumQuantity, maximumQuantity, \
             calculatedRequestedQuantity, lowStock, confirmed)."
                .into(),
        );
    }

    out.join("\n")
}

pub async fn insert_rnr_form(
    client: &OmSupplyClient,
    supplier_id: String,
    program_id: String,
    period_id: String,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let input = json!({
        "id": id,
        "supplierId": supplier_id,
        "programId": program_id,
        "periodId": period_id,
    });

    let data: Value = client
        .query(
            INSERT_RNR_FORM_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let node = data
        .get("insertRnrForm")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertRnrForm".into()))?;

    if node.get("__typename").and_then(|v| v.as_str()) != Some("RnRFormNode") {
        return Err(AppError::Graphql(format!(
            "insert failed: {node}"
        )));
    }

    Ok(format_rnr_form_with_lines(node, "R&R form created"))
}

pub async fn update_rnr_form(
    client: &OmSupplyClient,
    id: String,
    lines: Value,
    their_reference: Option<String>,
    comment: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    if !lines.is_array() {
        return Err(AppError::Graphql(
            "lines must be a JSON array of UpdateRnRFormLineInput objects".into(),
        ));
    }

    let mut input = json!({ "id": id, "lines": lines });
    if let Some(v) = their_reference {
        input["theirReference"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_RNR_FORM_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let node = data
        .get("updateRnrForm")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateRnrForm".into()))?;

    if node.get("__typename").and_then(|v| v.as_str()) != Some("RnRFormNode") {
        return Err(AppError::Graphql(format!("update failed: {node}")));
    }

    Ok(format_rnr_form_with_lines(node, "R&R form updated"))
}

pub async fn finalise_rnr_form(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            FINALISE_RNR_FORM_MUTATION,
            json!({ "storeId": resolved_store_id, "input": { "id": id } }),
        )
        .await?;

    let node = data
        .get("finaliseRnrForm")
        .ok_or_else(|| AppError::UnexpectedResponse("missing finaliseRnrForm".into()))?;

    if node.get("__typename").and_then(|v| v.as_str()) != Some("RnRFormNode") {
        return Err(AppError::Graphql(format!("finalise failed: {node}")));
    }

    Ok(format!("R&R form finalised:\n{}", format_record(node)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_rnr_form_with_lines_exposes_line_uuids() {
        let form = json!({
            "__typename": "RnRFormNode",
            "id": "de1a04b8-00ca-458e-96ba-fda508375bd6",
            "status": "DRAFT",
            "programName": "PRG-HIV",
            "supplierName": "Bedrock District Store",
            "comment": null,
            "lines": [
                {
                    "id": "line-uuid-aaa",
                    "itemId": "item-1",
                    "averageMonthlyConsumption": 12.5,
                    "initialBalance": 100.0,
                    "stockOutDuration": 0,
                    "lowStock": "OK",
                    "confirmed": false,
                    "item": { "id": "item-1", "code": "X-1", "name": "Aspirin", "unitName": "Tablet" }
                },
                {
                    "id": "line-uuid-bbb",
                    "itemId": "item-2",
                    "averageMonthlyConsumption": 0.0,
                    "initialBalance": 0.0,
                    "stockOutDuration": 5,
                    "lowStock": "BELOW_QUARTER",
                    "confirmed": true,
                    "item": { "id": "item-2", "code": "X-2", "name": "Bandage", "unitName": "Roll" }
                }
            ]
        });

        let out = format_rnr_form_with_lines(&form, "R&R form details");

        // Header fields rendered.
        assert!(out.contains("status: DRAFT"));
        assert!(out.contains("programName: PRG-HIV"));

        // Lines block renders, with both UUIDs and per-line content visible.
        assert!(out.contains("Lines (2 total):"));
        assert!(out.contains("--- Line 1 ---"));
        assert!(out.contains("--- Line 2 ---"));
        assert!(
            out.contains("id: line-uuid-aaa"),
            "expected line UUID 'line-uuid-aaa' in output, got:\n{out}"
        );
        assert!(
            out.contains("id: line-uuid-bbb"),
            "expected line UUID 'line-uuid-bbb' in output, got:\n{out}"
        );
        assert!(out.contains("itemId: item-1"));
        assert!(out.contains("itemId: item-2"));

        // Top-level lines should not be collapsed to "[N items]".
        assert!(
            !out.contains("lines: [2 items]"),
            "lines were collapsed instead of expanded:\n{out}"
        );
    }
}

pub async fn delete_rnr_form(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_RNR_FORM_MUTATION,
            json!({ "storeId": resolved_store_id, "input": { "id": id } }),
        )
        .await?;

    let node = data
        .get("deleteRnrForm")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteRnrForm".into()))?;

    if node.get("__typename").and_then(|v| v.as_str()) != Some("DeleteResponse") {
        return Err(AppError::Graphql(format!("delete failed: {node}")));
    }

    Ok(format!("R&R form deleted (id={id})"))
}
