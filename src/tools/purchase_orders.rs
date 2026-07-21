//! Purchase order tools — supplier orders.
//!
//! Status flow: NEW -> REQUEST_APPROVAL (optional) -> CONFIRMED -> SENT -> FINALISED.
//! Confirmed/Finalised transitions may require user permissions if the store
//! preference `purchase_order_must_be_authorised` is enabled.
//!
//! Settable date fields on update:
//!   - confirmedDatetime (NaiveDateTime)
//!   - sentDatetime (NaiveDateTime)
//!   - contractSignedDate, advancePaidDate, receivedAtPortDate,
//!     requestedDeliveryDate (NaiveDate)
//! Not settable: createdDatetime, finalisedDatetime, deliveredDatetime.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const LIST_QUERY: &str = r#"
  query purchaseOrders(
    $storeId: String!
    $first: Int
    $offset: Int
    $key: PurchaseOrderSortFieldInput!
    $desc: Boolean
    $filter: PurchaseOrderFilterInput
  ) {
    purchaseOrders(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
    ) {
      ... on PurchaseOrderConnector {
        __typename
        totalCount
        nodes {
          id number status supplier { id name } createdDatetime
          confirmedDatetime sentDatetime finalisedDatetime
          requestedDeliveryDate
          reference comment
        }
      }
    }
  }
"#;

const DETAIL_QUERY: &str = r#"
  query purchaseOrder($id: String!, $storeId: String!) {
    purchaseOrder(id: $id, storeId: $storeId) {
      ... on PurchaseOrderNode {
        __typename
        id number status reference comment
        supplier { id name code }
        createdDatetime confirmedDatetime sentDatetime finalisedDatetime
        contractSignedDate advancePaidDate receivedAtPortDate
        requestedDeliveryDate
        supplierDiscountPercentage supplierDiscountAmount
        currencyId foreignExchangeRate shippingMethod
        supplierAgent authorisingOfficer1 authorisingOfficer2
        additionalInstructions headingMessage
        agentCommission documentCharge communicationsCharge
        insuranceCharge freightCharge freightConditions
        lines {
          totalCount
          nodes {
            id item { id code name }
            requestedPackSize requestedNumberOfUnits
            adjustedNumberOfUnits receivedNumberOfUnits
            requestedDeliveryDate expectedDeliveryDate
            pricePerPackBeforeDiscount pricePerPackAfterDiscount
            note unit supplierItemCode comment status
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

const INSERT_MUTATION: &str = r#"
  mutation insertPurchaseOrder(
    $storeId: String!
    $input: InsertPurchaseOrderInput!
  ) {
    insertPurchaseOrder(storeId: $storeId, input: $input) {
      __typename
      ... on IdResponse { id }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updatePurchaseOrder(
    $storeId: String!
    $input: UpdatePurchaseOrderInput!
  ) {
    updatePurchaseOrder(storeId: $storeId, input: $input) {
      __typename
      ... on IdResponse { id }
      ... on UpdatePurchaseOrderError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deletePurchaseOrder($storeId: String!, $id: String!) {
    deletePurchaseOrder(storeId: $storeId, id: $id) {
      __typename
      ... on DeleteResponse { id }
      ... on DeletePurchaseOrderError {
        error { __typename description }
      }
    }
  }
"#;

const INSERT_LINE_MUTATION: &str = r#"
  mutation insertPurchaseOrderLine(
    $storeId: String!
    $input: InsertPurchaseOrderLineInput!
  ) {
    insertPurchaseOrderLine(storeId: $storeId, input: $input) {
      __typename
      ... on IdResponse { id }
      ... on InsertPurchaseOrderLineError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_LINE_MUTATION: &str = r#"
  mutation updatePurchaseOrderLine(
    $storeId: String!
    $input: UpdatePurchaseOrderLineInput!
  ) {
    updatePurchaseOrderLine(storeId: $storeId, input: $input) {
      __typename
      ... on IdResponse { id }
      ... on UpdatePurchaseOrderLineError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_LINES_MUTATION: &str = r#"
  mutation deletePurchaseOrderLines($storeId: String!, $ids: [String!]!) {
    deletePurchaseOrderLines(storeId: $storeId, ids: $ids) {
      id
      response {
        __typename
        ... on DeleteResponse { id }
        ... on DeletePurchaseOrderLineError {
          error { __typename description }
        }
      }
    }
  }
"#;

#[derive(Deserialize)]
struct ListResp {
    purchase_orders: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct DetailResp {
    purchase_order: Value,
}

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

// camelCase-aware deserialize for the response wrapper field naming.
impl ListResp {
    fn from(v: Value) -> Result<Self, AppError> {
        // The GraphQL field is `purchaseOrders` (camelCase) but we declared
        // `purchase_orders` with serde rename_all default snake. Easier to
        // grab the field by name directly.
        let nodes_val = v
            .get("purchaseOrders")
            .ok_or_else(|| AppError::UnexpectedResponse("missing purchaseOrders".into()))?;
        let conn: Connector = serde_json::from_value(nodes_val.clone())
            .map_err(|e| AppError::UnexpectedResponse(e.to_string()))?;
        Ok(Self { purchase_orders: conn })
    }
}

impl DetailResp {
    fn from(v: Value) -> Result<Self, AppError> {
        let n = v
            .get("purchaseOrder")
            .ok_or_else(|| AppError::UnexpectedResponse("missing purchaseOrder".into()))?
            .clone();
        Ok(Self { purchase_order: n })
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn list_purchase_orders(
    client: &OmSupplyClient,
    status: Option<String>,
    supplier_id: Option<String>,
    sort_by: Option<String>,
    desc: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(s) = status {
        filter.insert("status".into(), json!({ "equalTo": s }));
    }
    if let Some(s) = supplier_id {
        filter.insert("supplierId".into(), json!({ "equalTo": s }));
    }
    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let key = sort_by.unwrap_or_else(|| "createdDatetime".into());

    let raw: Value = client
        .query(
            LIST_QUERY,
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

    let data = ListResp::from(raw)?;
    Ok(format_list_result(
        "purchase orders",
        &data.purchase_orders.nodes,
        data.purchase_orders.total_count,
        first,
        offset,
    ))
}

pub async fn get_purchase_order(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let raw: Value = client
        .query(
            DETAIL_QUERY,
            json!({ "id": id, "storeId": resolved_store_id }),
        )
        .await?;

    let data = DetailResp::from(raw)?;
    let typename = data
        .purchase_order
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if typename != "PurchaseOrderNode" {
        let desc = data
            .purchase_order
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("purchase order not found");
        return Err(AppError::Graphql(desc.to_string()));
    }

    Ok(format!(
        "Purchase order details:\n{}",
        format_record(&data.purchase_order)
    ))
}

pub async fn insert_purchase_order(
    client: &OmSupplyClient,
    supplier_id: String,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let input = json!({ "id": id, "supplierId": supplier_id });

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertPurchaseOrder")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertPurchaseOrder".into()))?;
    let node = unwrap_mutation_response(response, "IdResponse")?;
    let new_id = node
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(&id);
    Ok(format!("Purchase order created (id={new_id})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_purchase_order(
    client: &OmSupplyClient,
    id: String,
    status: Option<String>,
    supplier_id: Option<String>,
    confirmed_datetime: Option<String>,
    sent_datetime: Option<String>,
    contract_signed_date: Option<String>,
    advance_paid_date: Option<String>,
    received_at_port_date: Option<String>,
    requested_delivery_date: Option<String>,
    comment: Option<String>,
    reference: Option<String>,
    supplier_discount_percentage: Option<f64>,
    supplier_discount_amount: Option<f64>,
    currency_id: Option<String>,
    foreign_exchange_rate: Option<f64>,
    shipping_method: Option<String>,
    donor_id: Option<String>,
    supplier_agent: Option<String>,
    authorising_officer_1: Option<String>,
    authorising_officer_2: Option<String>,
    additional_instructions: Option<String>,
    heading_message: Option<String>,
    agent_commission: Option<f64>,
    document_charge: Option<f64>,
    communications_charge: Option<f64>,
    insurance_charge: Option<f64>,
    freight_charge: Option<f64>,
    freight_conditions: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = status {
        input["status"] = json!(v);
    }
    if let Some(v) = supplier_id {
        input["supplierId"] = json!(v);
    }
    if let Some(v) = confirmed_datetime {
        input["confirmedDatetime"] = json!({ "value": v });
    }
    if let Some(v) = sent_datetime {
        input["sentDatetime"] = json!({ "value": v });
    }
    if let Some(v) = contract_signed_date {
        input["contractSignedDate"] = json!({ "value": v });
    }
    if let Some(v) = advance_paid_date {
        input["advancePaidDate"] = json!({ "value": v });
    }
    if let Some(v) = received_at_port_date {
        input["receivedAtPortDate"] = json!({ "value": v });
    }
    if let Some(v) = requested_delivery_date {
        input["requestedDeliveryDate"] = json!({ "value": v });
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = reference {
        input["reference"] = json!(v);
    }
    if let Some(v) = supplier_discount_percentage {
        input["supplierDiscountPercentage"] = json!(v);
    }
    if let Some(v) = supplier_discount_amount {
        input["supplierDiscountAmount"] = json!(v);
    }
    if let Some(v) = currency_id {
        input["currencyId"] = json!(v);
    }
    if let Some(v) = foreign_exchange_rate {
        input["foreignExchangeRate"] = json!(v);
    }
    if let Some(v) = shipping_method {
        input["shippingMethod"] = json!(v);
    }
    if let Some(v) = donor_id {
        input["donorId"] = json!({ "value": v });
    }
    if let Some(v) = supplier_agent {
        input["supplierAgent"] = json!(v);
    }
    if let Some(v) = authorising_officer_1 {
        input["authorisingOfficer1"] = json!(v);
    }
    if let Some(v) = authorising_officer_2 {
        input["authorisingOfficer2"] = json!(v);
    }
    if let Some(v) = additional_instructions {
        input["additionalInstructions"] = json!(v);
    }
    if let Some(v) = heading_message {
        input["headingMessage"] = json!(v);
    }
    if let Some(v) = agent_commission {
        input["agentCommission"] = json!(v);
    }
    if let Some(v) = document_charge {
        input["documentCharge"] = json!(v);
    }
    if let Some(v) = communications_charge {
        input["communicationsCharge"] = json!(v);
    }
    if let Some(v) = insurance_charge {
        input["insuranceCharge"] = json!(v);
    }
    if let Some(v) = freight_charge {
        input["freightCharge"] = json!(v);
    }
    if let Some(v) = freight_conditions {
        input["freightConditions"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updatePurchaseOrder")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updatePurchaseOrder".into()))?;
    unwrap_mutation_response(response, "IdResponse")?;
    Ok(format!("Purchase order updated (id={id})"))
}

pub async fn delete_purchase_order(
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
        .get("deletePurchaseOrder")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deletePurchaseOrder".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Purchase order deleted (id={id})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_purchase_order_line(
    client: &OmSupplyClient,
    purchase_order_id: String,
    item_id_or_code: String,
    requested_pack_size: Option<f64>,
    requested_number_of_units: Option<f64>,
    requested_delivery_date: Option<String>,
    expected_delivery_date: Option<String>,
    price_per_pack_before_discount: Option<f64>,
    price_per_pack_after_discount: Option<f64>,
    manufacturer_id: Option<String>,
    note: Option<String>,
    unit: Option<String>,
    supplier_item_code: Option<String>,
    comment: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({
        "id": id,
        "purchaseOrderId": purchase_order_id,
        "itemIdOrCode": item_id_or_code,
    });
    if let Some(v) = requested_pack_size {
        input["requestedPackSize"] = json!(v);
    }
    if let Some(v) = requested_number_of_units {
        input["requestedNumberOfUnits"] = json!(v);
    }
    if let Some(v) = requested_delivery_date {
        input["requestedDeliveryDate"] = json!(v);
    }
    if let Some(v) = expected_delivery_date {
        input["expectedDeliveryDate"] = json!(v);
    }
    if let Some(v) = price_per_pack_before_discount {
        input["pricePerPackBeforeDiscount"] = json!(v);
    }
    if let Some(v) = price_per_pack_after_discount {
        input["pricePerPackAfterDiscount"] = json!(v);
    }
    if let Some(v) = manufacturer_id {
        input["manufacturerId"] = json!(v);
    }
    if let Some(v) = note {
        input["note"] = json!(v);
    }
    if let Some(v) = unit {
        input["unit"] = json!(v);
    }
    if let Some(v) = supplier_item_code {
        input["supplierItemCode"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertPurchaseOrderLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertPurchaseOrderLine".into()))?;
    let node = unwrap_mutation_response(response, "IdResponse")?;
    let new_id = node.get("id").and_then(|v| v.as_str()).unwrap_or(&id);
    Ok(format!("Purchase order line created (id={new_id})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_purchase_order_line(
    client: &OmSupplyClient,
    id: String,
    item_id: Option<String>,
    requested_pack_size: Option<f64>,
    requested_number_of_units: Option<f64>,
    adjusted_number_of_units: Option<f64>,
    requested_delivery_date: Option<String>,
    expected_delivery_date: Option<String>,
    price_per_pack_before_discount: Option<f64>,
    price_per_pack_after_discount: Option<f64>,
    manufacturer_id: Option<String>,
    note: Option<String>,
    unit: Option<String>,
    supplier_item_code: Option<String>,
    comment: Option<String>,
    status: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = item_id {
        input["itemId"] = json!(v);
    }
    if let Some(v) = requested_pack_size {
        input["requestedPackSize"] = json!(v);
    }
    if let Some(v) = requested_number_of_units {
        input["requestedNumberOfUnits"] = json!(v);
    }
    if let Some(v) = adjusted_number_of_units {
        input["adjustedNumberOfUnits"] = json!(v);
    }
    if let Some(v) = requested_delivery_date {
        input["requestedDeliveryDate"] = json!({ "value": v });
    }
    if let Some(v) = expected_delivery_date {
        input["expectedDeliveryDate"] = json!({ "value": v });
    }
    if let Some(v) = price_per_pack_before_discount {
        input["pricePerPackBeforeDiscount"] = json!(v);
    }
    if let Some(v) = price_per_pack_after_discount {
        input["pricePerPackAfterDiscount"] = json!(v);
    }
    if let Some(v) = manufacturer_id {
        input["manufacturerId"] = json!({ "value": v });
    }
    if let Some(v) = note {
        input["note"] = json!({ "value": v });
    }
    if let Some(v) = unit {
        input["unit"] = json!(v);
    }
    if let Some(v) = supplier_item_code {
        input["supplierItemCode"] = json!({ "value": v });
    }
    if let Some(v) = comment {
        input["comment"] = json!({ "value": v });
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
        .get("updatePurchaseOrderLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updatePurchaseOrderLine".into()))?;
    unwrap_mutation_response(response, "IdResponse")?;
    Ok(format!("Purchase order line updated (id={id})"))
}

pub async fn delete_purchase_order_lines(
    client: &OmSupplyClient,
    ids: Vec<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: Value = client
        .query(
            DELETE_LINES_MUTATION,
            json!({ "storeId": resolved_store_id, "ids": ids }),
        )
        .await?;

    let results = data
        .get("deletePurchaseOrderLines")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::UnexpectedResponse("missing deletePurchaseOrderLines".into()))?;

    let mut summary = vec![format!("Deleted {} line(s):", results.len())];
    for entry in results {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let response = entry.get("response").cloned().unwrap_or(Value::Null);
        match unwrap_mutation_response(&response, "DeleteResponse") {
            Ok(_) => summary.push(format!("  {id}: ok")),
            Err(e) => summary.push(format!("  {id}: {e}")),
        }
    }
    Ok(summary.join("\n"))
}
