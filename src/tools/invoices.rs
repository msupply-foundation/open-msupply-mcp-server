//! Invoice tools -- port of src/tools/invoices.ts

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const INVOICES_QUERY: &str = r#"
  query invoices(
    $first: Int
    $offset: Int
    $key: InvoiceSortFieldInput!
    $desc: Boolean
    $filter: InvoiceFilterInput
    $storeId: String!
  ) {
    invoices(
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
      storeId: $storeId
    ) {
      ... on InvoiceConnector {
        __typename
        totalCount
        nodes {
          id invoiceNumber type status otherPartyName createdDatetime
          allocatedDatetime shippedDatetime deliveredDatetime
          comment theirReference colour
          pricing { totalAfterTax }
        }
      }
    }
  }
"#;

const INVOICE_DETAIL_QUERY: &str = r#"
  query invoice($id: String!, $storeId: String!) {
    invoice(id: $id, storeId: $storeId) {
      ... on InvoiceNode {
        __typename
        id invoiceNumber type status otherPartyName
        purchaseOrderId
        createdDatetime allocatedDatetime pickedDatetime
        shippedDatetime deliveredDatetime verifiedDatetime
        comment theirReference transportReference colour taxPercentage
        pricing { totalAfterTax taxPercentage }
        lines {
          totalCount
          nodes {
            id type numberOfPacks packSize
            costPricePerPack sellPricePerPack batch expiryDate
            item { id code name unitName }
          }
        }
        otherParty(storeId: $storeId) { id name code isCustomer isSupplier }
      }
      ... on NodeError {
        __typename
        error { description }
      }
    }
  }
"#;

const OUTBOUND_COUNTS_QUERY: &str = r#"
  query outboundShipmentCounts($storeId: String!) {
    outboundShipmentCounts(storeId: $storeId) {
      created { today thisWeek }
      notShipped
    }
  }
"#;

const INBOUND_COUNTS_QUERY: &str = r#"
  query inboundShipmentCounts($storeId: String!) {
    inboundShipmentCounts(storeId: $storeId) {
      created { today thisWeek }
      notDelivered
    }
  }
"#;

#[derive(Deserialize)]
struct InvoicesResp {
    invoices: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct InvoiceDetailResp {
    invoice: Value,
}

#[derive(Deserialize)]
struct OutboundResp {
    #[serde(rename = "outboundShipmentCounts")]
    outbound_shipment_counts: ShipCounts,
}

#[derive(Deserialize)]
struct InboundResp {
    #[serde(rename = "inboundShipmentCounts")]
    inbound_shipment_counts: InboundShipCounts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Created {
    today: u32,
    this_week: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShipCounts {
    created: Created,
    not_shipped: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboundShipCounts {
    created: Created,
    not_delivered: u32,
}

#[allow(clippy::too_many_arguments)]
pub async fn list_invoices(
    client: &OmSupplyClient,
    invoice_type: Option<String>,
    status: Option<String>,
    other_party_name: Option<String>,
    sort_by: Option<String>,
    desc: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    // Filter type via the standard `filter.type.equalTo` rather than the
    // top-level `type` argument: at the invoice level a purchase-order-linked
    // inbound is still an INBOUND_SHIPMENT (there is no separate invoice type
    // for it), so equalTo INBOUND_SHIPMENT includes PO-linked inbounds too.
    if let Some(t) = invoice_type {
        filter.insert("type".into(), json!({ "equalTo": t }));
    }
    if let Some(s) = status {
        filter.insert("status".into(), json!({ "equalTo": s }));
    }
    if let Some(n) = other_party_name {
        filter.insert("otherPartyName".into(), json!({ "like": n }));
    }

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let key = sort_by.unwrap_or_else(|| "createdDatetime".into());

    let data: InvoicesResp = client
        .query(
            INVOICES_QUERY,
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
        "invoices",
        &data.invoices.nodes,
        data.invoices.total_count,
        first,
        offset,
    ))
}

pub async fn get_invoice(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: InvoiceDetailResp = client
        .query(
            INVOICE_DETAIL_QUERY,
            json!({ "id": id, "storeId": resolved_store_id }),
        )
        .await?;

    let typename = data
        .invoice
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if typename == "NodeError" {
        let desc = data
            .invoice
            .pointer("/error/description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(AppError::Graphql(desc.to_string()));
    }

    Ok(format!("Invoice details:\n{}", format_record(&data.invoice)))
}

pub async fn get_outbound_shipment_counts(
    client: &OmSupplyClient,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let data: OutboundResp = client
        .query(OUTBOUND_COUNTS_QUERY, json!({ "storeId": resolved_store_id }))
        .await?;
    let c = data.outbound_shipment_counts;
    Ok(format!(
        "Outbound Shipment Counts:\n  Created today: {}\n  Created this week: {}\n  Not yet shipped: {}",
        c.created.today, c.created.this_week, c.not_shipped
    ))
}

pub async fn get_inbound_shipment_counts(
    client: &OmSupplyClient,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let data: InboundResp = client
        .query(INBOUND_COUNTS_QUERY, json!({ "storeId": resolved_store_id }))
        .await?;
    let c = data.inbound_shipment_counts;
    Ok(format!(
        "Inbound Shipment Counts:\n  Created today: {}\n  Created this week: {}\n  Not yet delivered: {}",
        c.created.today, c.created.this_week, c.not_delivered
    ))
}
