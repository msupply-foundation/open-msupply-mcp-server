//! Requisition tools — request requisitions read + write.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const REQUISITIONS_QUERY: &str = r#"
  query requisitions(
    $first: Int
    $offset: Int
    $key: RequisitionSortFieldInput!
    $desc: Boolean
    $filter: RequisitionFilterInput
    $storeId: String!
  ) {
    requisitions(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
    ) {
      ... on RequisitionConnector {
        __typename
        totalCount
        nodes {
          id requisitionNumber type status otherPartyName
          createdDatetime sentDatetime finalisedDatetime expectedDeliveryDate
          comment theirReference colour orderType isEmergency
          maxMonthsOfStock minMonthsOfStock
          program { id name }
          period { id name startDate endDate }
        }
      }
    }
  }
"#;

const REQUISITION_DETAIL_QUERY: &str = r#"
  query requisition($id: String!, $storeId: String!) {
    requisition(id: $id, storeId: $storeId) {
      ... on RequisitionNode {
        __typename
        id requisitionNumber type status otherPartyName otherPartyId
        createdDatetime sentDatetime finalisedDatetime expectedDeliveryDate
        comment theirReference colour orderType isEmergency
        maxMonthsOfStock minMonthsOfStock approvalStatus
        program { id name }
        period { id name startDate endDate }
        otherParty(storeId: $storeId) { id name code isCustomer isSupplier }
        lines {
          totalCount
          nodes {
            id itemId itemName requestedQuantity supplyQuantity
            suggestedQuantity approvedQuantity comment
            availableStockOnHand averageMonthlyConsumption
            item { id code name unitName }
          }
        }
      }
      ... on RecordNotFound {
        __typename
        description
      }
    }
  }
"#;

const INSERT_REQUEST_REQUISITION_MUTATION: &str = r#"
  mutation insertRequestRequisition(
    $storeId: String!
    $input: InsertRequestRequisitionInput!
  ) {
    insertRequestRequisition(storeId: $storeId, input: $input) {
      __typename
      ... on RequisitionNode {
        id requisitionNumber type status otherPartyName createdDatetime
      }
      ... on InsertRequestRequisitionError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_REQUEST_REQUISITION_MUTATION: &str = r#"
  mutation updateRequestRequisition(
    $storeId: String!
    $input: UpdateRequestRequisitionInput!
  ) {
    updateRequestRequisition(storeId: $storeId, input: $input) {
      __typename
      ... on RequisitionNode {
        id requisitionNumber type status sentDatetime finalisedDatetime comment theirReference
      }
      ... on UpdateRequestRequisitionError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_REQUEST_REQUISITION_MUTATION: &str = r#"
  mutation deleteRequestRequisition(
    $storeId: String!
    $input: DeleteRequestRequisitionInput!
  ) {
    deleteRequestRequisition(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteRequestRequisitionError {
        error { __typename description }
      }
    }
  }
"#;

const INSERT_REQUEST_REQUISITION_LINE_MUTATION: &str = r#"
  mutation insertRequestRequisitionLine(
    $storeId: String!
    $input: InsertRequestRequisitionLineInput!
  ) {
    insertRequestRequisitionLine(storeId: $storeId, input: $input) {
      __typename
      ... on RequisitionLineNode {
        id itemId itemName requestedQuantity suggestedQuantity comment
      }
      ... on InsertRequestRequisitionLineError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_REQUEST_REQUISITION_LINE_MUTATION: &str = r#"
  mutation updateRequestRequisitionLine(
    $storeId: String!
    $input: UpdateRequestRequisitionLineInput!
  ) {
    updateRequestRequisitionLine(storeId: $storeId, input: $input) {
      __typename
      ... on RequisitionLineNode {
        id itemId itemName requestedQuantity suggestedQuantity comment
      }
      ... on UpdateRequestRequisitionLineError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_REQUEST_REQUISITION_LINE_MUTATION: &str = r#"
  mutation deleteRequestRequisitionLine(
    $storeId: String!
    $input: DeleteRequestRequisitionLineInput!
  ) {
    deleteRequestRequisitionLine(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteRequestRequisitionLineError {
        error { __typename description }
      }
    }
  }
"#;

#[derive(Deserialize)]
struct RequisitionsResp {
    requisitions: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct RequisitionDetailResp {
    requisition: Value,
}

#[allow(clippy::too_many_arguments)]
pub async fn list_requisitions(
    client: &OmSupplyClient,
    requisition_type: Option<String>,
    status: Option<String>,
    other_party_name: Option<String>,
    program_id: Option<String>,
    is_emergency: Option<bool>,
    sort_by: Option<String>,
    desc: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(t) = requisition_type {
        filter.insert("type".into(), json!({ "equalTo": t }));
    }
    if let Some(s) = status {
        filter.insert("status".into(), json!({ "equalTo": s }));
    }
    if let Some(n) = other_party_name {
        filter.insert("otherPartyName".into(), json!({ "like": n }));
    }
    if let Some(p) = program_id {
        filter.insert("programId".into(), json!({ "equalTo": p }));
    }
    if let Some(b) = is_emergency {
        filter.insert("isEmergency".into(), Value::Bool(b));
    }

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let key = sort_by.unwrap_or_else(|| "createdDatetime".into());

    let data: RequisitionsResp = client
        .query(
            REQUISITIONS_QUERY,
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
        "requisitions",
        &data.requisitions.nodes,
        data.requisitions.total_count,
        first,
        offset,
    ))
}

pub async fn get_requisition(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: RequisitionDetailResp = client
        .query(
            REQUISITION_DETAIL_QUERY,
            json!({ "id": id, "storeId": resolved_store_id }),
        )
        .await?;

    let typename = data
        .requisition
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if typename != "RequisitionNode" {
        let desc = data
            .requisition
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("requisition not found");
        return Err(AppError::Graphql(desc.to_string()));
    }

    Ok(format!(
        "Requisition details:\n{}",
        format_record(&data.requisition)
    ))
}

/// Read the typename + describe an error from a mutation union response.
/// Returns Ok(node) if the response is the success node, Err otherwise.
fn unwrap_mutation_response(
    response: &Value,
    success_typename: &str,
) -> Result<Value, AppError> {
    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if typename == success_typename {
        return Ok(response.clone());
    }

    let err_typename = response
        .pointer("/error/__typename")
        .and_then(|v| v.as_str())
        .unwrap_or(typename);
    let desc = response
        .pointer("/error/description")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    Err(AppError::Graphql(format!("{err_typename}: {desc}")))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_request_requisition(
    client: &OmSupplyClient,
    other_party_id: String,
    max_months_of_stock: f64,
    min_months_of_stock: f64,
    their_reference: Option<String>,
    comment: Option<String>,
    colour: Option<String>,
    expected_delivery_date: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({
        "id": id,
        "otherPartyId": other_party_id,
        "maxMonthsOfStock": max_months_of_stock,
        "minMonthsOfStock": min_months_of_stock,
    });
    if let Some(v) = their_reference {
        input["theirReference"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = colour {
        input["colour"] = json!(v);
    }
    if let Some(v) = expected_delivery_date {
        input["expectedDeliveryDate"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_REQUEST_REQUISITION_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertRequestRequisition")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertRequestRequisition".into()))?;
    let node = unwrap_mutation_response(response, "RequisitionNode")?;
    Ok(format!(
        "Request requisition created:\n{}",
        format_record(&node)
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_request_requisition(
    client: &OmSupplyClient,
    id: String,
    status: Option<String>,
    comment: Option<String>,
    their_reference: Option<String>,
    colour: Option<String>,
    other_party_id: Option<String>,
    expected_delivery_date: Option<String>,
    max_months_of_stock: Option<f64>,
    min_months_of_stock: Option<f64>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = status {
        input["status"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = their_reference {
        input["theirReference"] = json!(v);
    }
    if let Some(v) = colour {
        input["colour"] = json!(v);
    }
    if let Some(v) = other_party_id {
        input["otherPartyId"] = json!(v);
    }
    if let Some(v) = expected_delivery_date {
        input["expectedDeliveryDate"] = json!(v);
    }
    if let Some(v) = max_months_of_stock {
        input["maxMonthsOfStock"] = json!(v);
    }
    if let Some(v) = min_months_of_stock {
        input["minMonthsOfStock"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_REQUEST_REQUISITION_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateRequestRequisition")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateRequestRequisition".into()))?;
    let node = unwrap_mutation_response(response, "RequisitionNode")?;
    Ok(format!(
        "Request requisition updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_request_requisition(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_REQUEST_REQUISITION_MUTATION,
            json!({ "storeId": resolved_store_id, "input": { "id": id } }),
        )
        .await?;

    let response = data
        .get("deleteRequestRequisition")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteRequestRequisition".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Request requisition deleted (id={id})"))
}

pub async fn insert_request_requisition_line(
    client: &OmSupplyClient,
    requisition_id: String,
    item_id: String,
    requested_quantity: Option<f64>,
    comment: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let line_id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let input = json!({
        "id": line_id,
        "itemId": item_id,
        "requisitionId": requisition_id,
    });

    let data: Value = client
        .query(
            INSERT_REQUEST_REQUISITION_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertRequestRequisitionLine")
        .ok_or_else(|| {
            AppError::UnexpectedResponse("missing insertRequestRequisitionLine".into())
        })?;
    let node = unwrap_mutation_response(response, "RequisitionLineNode")?;

    // The insert mutation does not accept requested_quantity or comment directly,
    // so call update afterwards if either was provided.
    if requested_quantity.is_some() || comment.is_some() {
        return update_request_requisition_line(
            client,
            line_id,
            requested_quantity,
            comment,
            None,
            Some(resolved_store_id),
        )
        .await;
    }

    Ok(format!(
        "Request requisition line created:\n{}",
        format_record(&node)
    ))
}

pub async fn update_request_requisition_line(
    client: &OmSupplyClient,
    id: String,
    requested_quantity: Option<f64>,
    comment: Option<String>,
    option_id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = requested_quantity {
        input["requestedQuantity"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = option_id {
        input["optionId"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_REQUEST_REQUISITION_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateRequestRequisitionLine")
        .ok_or_else(|| {
            AppError::UnexpectedResponse("missing updateRequestRequisitionLine".into())
        })?;
    let node = unwrap_mutation_response(response, "RequisitionLineNode")?;
    Ok(format!(
        "Request requisition line updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_request_requisition_line(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_REQUEST_REQUISITION_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": { "id": id } }),
        )
        .await?;

    let response = data
        .get("deleteRequestRequisitionLine")
        .ok_or_else(|| {
            AppError::UnexpectedResponse("missing deleteRequestRequisitionLine".into())
        })?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Request requisition line deleted (id={id})"))
}
