//! Inbound shipment tools — invoices coming INTO this store (supplier deliveries).
//!
//! In Open mSupply, "goods receipts" are represented as inbound shipments.
//!
//! Status flow: NEW -> SHIPPED -> DELIVERED -> RECEIVED -> VERIFIED.
//! Backdating is limited: only `receivedDatetime` is directly settable. Other
//! status timestamps (delivered, verified) are server-stamped at "now" during
//! their transitions — there is no GraphQL override for them. Insert lines
//! support `expiryDate` and `manufactureDate` for realistic batch ageing.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::format_record;
use serde_json::{Value, json};
use uuid::Uuid;

const INSERT_MUTATION: &str = r#"
  mutation insertInboundShipment(
    $storeId: String!
    $input: InsertInboundShipmentInput!
  ) {
    insertInboundShipment(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceNode {
        id invoiceNumber status type otherPartyName createdDatetime comment
      }
      ... on InsertInboundShipmentError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updateInboundShipment(
    $storeId: String!
    $input: UpdateInboundShipmentInput!
  ) {
    updateInboundShipment(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceNode {
        id invoiceNumber status otherPartyName
        createdDatetime shippedDatetime deliveredDatetime receivedDatetime verifiedDatetime
        comment theirReference
      }
      ... on UpdateInboundShipmentError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deleteInboundShipment(
    $storeId: String!
    $input: DeleteInboundShipmentInput!
  ) {
    deleteInboundShipment(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteInboundShipmentError {
        error { __typename description }
      }
    }
  }
"#;

const INSERT_LINE_MUTATION: &str = r#"
  mutation insertInboundShipmentLine(
    $storeId: String!
    $input: InsertInboundShipmentLineInput!
  ) {
    insertInboundShipmentLine(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceLineNode {
        id itemName itemCode batch numberOfPacks packSize
        sellPricePerPack costPricePerPack expiryDate
      }
      ... on InsertInboundShipmentLineError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_LINE_MUTATION: &str = r#"
  mutation updateInboundShipmentLine(
    $storeId: String!
    $input: UpdateInboundShipmentLineInput!
  ) {
    updateInboundShipmentLine(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceLineNode {
        id itemName itemCode batch numberOfPacks packSize
        sellPricePerPack costPricePerPack expiryDate
      }
      ... on UpdateInboundShipmentLineError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_LINE_MUTATION: &str = r#"
  mutation deleteInboundShipmentLine(
    $storeId: String!
    $input: DeleteInboundShipmentLineInput!
  ) {
    deleteInboundShipmentLine(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteInboundShipmentLineError {
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
pub async fn insert_inbound_shipment(
    client: &OmSupplyClient,
    other_party_id: String,
    on_hold: Option<bool>,
    comment: Option<String>,
    their_reference: Option<String>,
    colour: Option<String>,
    requisition_id: Option<String>,
    purchase_order_id: Option<String>,
    insert_lines_from_purchase_order: Option<bool>,
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
    if let Some(v) = requisition_id {
        input["requisitionId"] = json!(v);
    }
    if let Some(v) = purchase_order_id {
        input["purchaseOrderId"] = json!(v);
    }
    if let Some(v) = insert_lines_from_purchase_order {
        input["insertLinesFromPurchaseOrder"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertInboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertInboundShipment".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceNode")?;
    Ok(format!(
        "Inbound shipment created:\n{}",
        format_record(&node)
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_inbound_shipment(
    client: &OmSupplyClient,
    id: String,
    status: Option<String>,
    on_hold: Option<bool>,
    comment: Option<String>,
    their_reference: Option<String>,
    colour: Option<String>,
    other_party_id: Option<String>,
    received_datetime: Option<String>,
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
    if let Some(v) = colour {
        input["colour"] = json!(v);
    }
    if let Some(v) = other_party_id {
        input["otherPartyId"] = json!(v);
    }
    if let Some(v) = received_datetime {
        input["receivedDatetime"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateInboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateInboundShipment".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceNode")?;
    Ok(format!(
        "Inbound shipment updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_inbound_shipment(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": { "id": id } }),
        )
        .await?;

    let response = data
        .get("deleteInboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteInboundShipment".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Inbound shipment deleted (id={id})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_inbound_shipment_line(
    client: &OmSupplyClient,
    invoice_id: String,
    item_id: String,
    pack_size: f64,
    number_of_packs: f64,
    cost_price_per_pack: f64,
    sell_price_per_pack: f64,
    batch: Option<String>,
    expiry_date: Option<String>,
    manufacture_date: Option<String>,
    location_id: Option<String>,
    note: Option<String>,
    tax_percentage: Option<f64>,
    total_before_tax: Option<f64>,
    item_variant_id: Option<String>,
    vvm_status_id: Option<String>,
    donor_id: Option<String>,
    manufacturer_id: Option<String>,
    campaign_id: Option<String>,
    program_id: Option<String>,
    purchase_order_line_id: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({
        "id": id,
        "invoiceId": invoice_id,
        "itemId": item_id,
        "packSize": pack_size,
        "numberOfPacks": number_of_packs,
        "costPricePerPack": cost_price_per_pack,
        "sellPricePerPack": sell_price_per_pack,
    });
    if let Some(v) = batch {
        input["batch"] = json!(v);
    }
    if let Some(v) = expiry_date {
        input["expiryDate"] = json!(v);
    }
    if let Some(v) = manufacture_date {
        input["manufactureDate"] = json!(v);
    }
    if let Some(v) = location_id {
        input["location"] = json!({ "value": v });
    }
    if let Some(v) = note {
        input["note"] = json!(v);
    }
    if let Some(v) = tax_percentage {
        input["taxPercentage"] = json!(v);
    }
    if let Some(v) = total_before_tax {
        input["totalBeforeTax"] = json!(v);
    }
    if let Some(v) = item_variant_id {
        input["itemVariantId"] = json!(v);
    }
    if let Some(v) = vvm_status_id {
        input["vvmStatusId"] = json!(v);
    }
    if let Some(v) = donor_id {
        input["donorId"] = json!(v);
    }
    if let Some(v) = manufacturer_id {
        input["manufacturerId"] = json!(v);
    }
    if let Some(v) = campaign_id {
        input["campaignId"] = json!(v);
    }
    if let Some(v) = program_id {
        input["programId"] = json!(v);
    }
    if let Some(v) = purchase_order_line_id {
        input["purchaseOrderLineId"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertInboundShipmentLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertInboundShipmentLine".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceLineNode")?;
    Ok(format!(
        "Inbound shipment line created:\n{}",
        format_record(&node)
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_inbound_shipment_line(
    client: &OmSupplyClient,
    id: String,
    item_id: Option<String>,
    pack_size: Option<f64>,
    number_of_packs: Option<f64>,
    cost_price_per_pack: Option<f64>,
    sell_price_per_pack: Option<f64>,
    batch: Option<String>,
    expiry_date: Option<String>,
    manufacture_date: Option<String>,
    location_id: Option<String>,
    note: Option<String>,
    status: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = item_id {
        input["itemId"] = json!(v);
    }
    if let Some(v) = pack_size {
        input["packSize"] = json!(v);
    }
    if let Some(v) = number_of_packs {
        input["numberOfPacks"] = json!(v);
    }
    if let Some(v) = cost_price_per_pack {
        input["costPricePerPack"] = json!(v);
    }
    if let Some(v) = sell_price_per_pack {
        input["sellPricePerPack"] = json!(v);
    }
    if let Some(v) = batch {
        input["batch"] = json!(v);
    }
    if let Some(v) = expiry_date {
        input["expiryDate"] = json!({ "value": v });
    }
    if let Some(v) = manufacture_date {
        input["manufactureDate"] = json!({ "value": v });
    }
    if let Some(v) = location_id {
        input["location"] = json!({ "value": v });
    }
    if let Some(v) = note {
        input["note"] = json!({ "value": v });
    }
    if let Some(v) = status {
        input["status"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateInboundShipmentLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateInboundShipmentLine".into()))?;
    let node = unwrap_mutation_response(response, "InvoiceLineNode")?;
    Ok(format!(
        "Inbound shipment line updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_inbound_shipment_line(
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
        .get("deleteInboundShipmentLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteInboundShipmentLine".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Inbound shipment line deleted (id={id})"))
}
