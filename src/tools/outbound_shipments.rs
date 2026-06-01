//! Outbound shipment tools — invoices going OUT of this store (customer shipments).
//!
//! Status flow: NEW -> ALLOCATED -> PICKED -> SHIPPED (status cannot reverse).
//! Backdating: pass `backdatedDatetime` on update to make the whole shipment appear
//! historic. The service walks the status timestamps back from that value, so a
//! single update call with `status: SHIPPED, backdatedDatetime: <past>` produces
//! a complete historical shipment.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::format_record;
use serde_json::{Value, json};
use uuid::Uuid;

const INSERT_MUTATION: &str = r#"
  mutation insertOutboundShipment(
    $storeId: String!
    $input: InsertOutboundShipmentInput!
  ) {
    insertOutboundShipment(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceNode {
        id invoiceNumber status type otherPartyName createdDatetime comment
      }
      ... on InsertOutboundShipmentError {
        error { __typename description }
      }
      ... on NodeError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updateOutboundShipment(
    $storeId: String!
    $input: UpdateOutboundShipmentInput!
  ) {
    updateOutboundShipment(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceNode {
        id invoiceNumber status otherPartyName
        createdDatetime allocatedDatetime pickedDatetime shippedDatetime
        deliveredDatetime verifiedDatetime backdatedDatetime expectedDeliveryDate
        comment theirReference transportReference
      }
      ... on UpdateOutboundShipmentError {
        error { __typename description }
      }
      ... on NodeError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deleteOutboundShipment($storeId: String!, $id: String!) {
    deleteOutboundShipment(storeId: $storeId, id: $id) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteOutboundShipmentError {
        error { __typename description }
      }
    }
  }
"#;

const INSERT_LINE_MUTATION: &str = r#"
  mutation insertOutboundShipmentLine(
    $storeId: String!
    $input: InsertOutboundShipmentLineInput!
  ) {
    insertOutboundShipmentLine(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceLineNode {
        id itemName itemCode batch numberOfPacks packSize
        sellPricePerPack costPricePerPack expiryDate locationName
      }
      ... on InsertOutboundShipmentLineError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_LINE_MUTATION: &str = r#"
  mutation updateOutboundShipmentLine(
    $storeId: String!
    $input: UpdateOutboundShipmentLineInput!
  ) {
    updateOutboundShipmentLine(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceLineNode {
        id itemName itemCode batch numberOfPacks packSize
        sellPricePerPack costPricePerPack expiryDate
      }
      ... on UpdateOutboundShipmentLineError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_LINE_MUTATION: &str = r#"
  mutation deleteOutboundShipmentLine(
    $storeId: String!
    $input: DeleteOutboundShipmentLineInput!
  ) {
    deleteOutboundShipmentLine(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteOutboundShipmentLineError {
        error { __typename description }
      }
    }
  }
"#;

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
pub async fn insert_outbound_shipment(
    client: &OmSupplyClient,
    other_party_id: String,
    on_hold: Option<bool>,
    comment: Option<String>,
    their_reference: Option<String>,
    colour: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({ "id": id, "otherPartyId": other_party_id });
    if let Some(v) = on_hold {
        input["onHold"] = json!(v);
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

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertOutboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertOutboundShipment".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceNode")?;
    Ok(format!(
        "Outbound shipment created:\n{}",
        format_record(&node)
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_outbound_shipment(
    client: &OmSupplyClient,
    id: String,
    status: Option<String>,
    on_hold: Option<bool>,
    comment: Option<String>,
    their_reference: Option<String>,
    transport_reference: Option<String>,
    colour: Option<String>,
    expected_delivery_date: Option<String>,
    backdated_datetime: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = status {
        input["status"] = json!(v);
    }
    if let Some(v) = on_hold {
        input["onHold"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = their_reference {
        input["theirReference"] = json!(v);
    }
    if let Some(v) = transport_reference {
        input["transportReference"] = json!(v);
    }
    if let Some(v) = colour {
        input["colour"] = json!(v);
    }
    if let Some(v) = expected_delivery_date {
        input["expectedDeliveryDate"] = json!({ "value": v });
    }
    if let Some(v) = backdated_datetime {
        input["backdatedDatetime"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateOutboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateOutboundShipment".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceNode")?;
    Ok(format!(
        "Outbound shipment updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_outbound_shipment(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_MUTATION,
            json!({ "storeId": resolved_store_id, "id": id }),
        )
        .await?;

    let response = data
        .get("deleteOutboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteOutboundShipment".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Outbound shipment deleted (id={id})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_outbound_shipment_line(
    client: &OmSupplyClient,
    invoice_id: String,
    stock_line_id: String,
    number_of_packs: f64,
    tax_percentage: Option<f64>,
    vvm_status_id: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({
        "id": id,
        "invoiceId": invoice_id,
        "stockLineId": stock_line_id,
        "numberOfPacks": number_of_packs,
    });
    if let Some(v) = tax_percentage {
        input["taxPercentage"] = json!(v);
    }
    if let Some(v) = vvm_status_id {
        input["vvmStatusId"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertOutboundShipmentLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertOutboundShipmentLine".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceLineNode")?;
    Ok(format!(
        "Outbound shipment line created:\n{}",
        format_record(&node)
    ))
}

pub async fn update_outbound_shipment_line(
    client: &OmSupplyClient,
    id: String,
    stock_line_id: Option<String>,
    number_of_packs: Option<f64>,
    prescribed_quantity: Option<f64>,
    vvm_status_id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = stock_line_id {
        input["stockLineId"] = json!(v);
    }
    if let Some(v) = number_of_packs {
        input["numberOfPacks"] = json!(v);
    }
    if let Some(v) = prescribed_quantity {
        input["prescribedQuantity"] = json!(v);
    }
    if let Some(v) = vvm_status_id {
        input["vvmStatusId"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateOutboundShipmentLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateOutboundShipmentLine".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceLineNode")?;
    Ok(format!(
        "Outbound shipment line updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_outbound_shipment_line(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": { "id": id } }),
        )
        .await?;

    let response = data
        .get("deleteOutboundShipmentLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteOutboundShipmentLine".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Outbound shipment line deleted (id={id})"))
}
