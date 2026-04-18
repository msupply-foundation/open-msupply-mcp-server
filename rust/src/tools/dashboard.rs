//! Dashboard + names + master lists tools -- port of src/tools/dashboard.ts

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const DASHBOARD_QUERY: &str = r#"
  query dashboard($storeId: String!) {
    stockCounts(storeId: $storeId, daysTillExpired: 30) {
      expired
      expiringSoon
    }
    itemCounts(storeId: $storeId, lowStockThreshold: 3, highStockThreshold: 6) {
      itemCounts { lowStock noStock highStock total }
    }
    outboundShipmentCounts(storeId: $storeId) {
      created { today thisWeek }
      notShipped
    }
    inboundShipmentCounts(storeId: $storeId) {
      created { today thisWeek }
      notDelivered
    }
    requisitionCounts(storeId: $storeId) {
      request { draft }
      response { new }
    }
  }
"#;

const REQUISITION_COUNTS_QUERY: &str = r#"
  query requisitionCounts($storeId: String!) {
    requisitionCounts(storeId: $storeId) {
      request { draft }
      response { new }
      emergency { new }
    }
  }
"#;

const NAMES_QUERY: &str = r#"
  query names(
    $storeId: String!
    $key: NameSortFieldInput!
    $desc: Boolean
    $first: Int
    $offset: Int
    $filter: NameFilterInput
  ) {
    names(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
    ) {
      ... on NameConnector {
        __typename
        totalCount
        nodes {
          id code name isCustomer isSupplier isOnHold
          store { id code }
        }
      }
    }
  }
"#;

const MASTER_LISTS_QUERY: &str = r#"
  query masterLists(
    $storeId: String!
    $first: Int
    $offset: Int
    $filter: MasterListFilterInput
    $sort: [MasterListSortInput!]
  ) {
    masterLists(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      filter: $filter
      sort: $sort
    ) {
      ... on MasterListConnector {
        __typename
        totalCount
        nodes { id code name description linesCount }
      }
    }
  }
"#;

#[derive(Deserialize)]
struct DashboardResp {
    #[serde(rename = "stockCounts")]
    stock_counts: StockCountsInner,
    #[serde(rename = "itemCounts")]
    item_counts: ItemCountsOuter,
    #[serde(rename = "outboundShipmentCounts")]
    outbound_shipment_counts: OutboundShip,
    #[serde(rename = "inboundShipmentCounts")]
    inbound_shipment_counts: InboundShip,
    #[serde(rename = "requisitionCounts")]
    requisition_counts: DashReqCounts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StockCountsInner {
    expired: u32,
    expiring_soon: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemCountsOuter {
    item_counts: ItemCountsInner,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemCountsInner {
    low_stock: u32,
    no_stock: u32,
    high_stock: u32,
    total: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Created {
    today: u32,
    this_week: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutboundShip {
    created: Created,
    not_shipped: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboundShip {
    created: Created,
    not_delivered: u32,
}

#[derive(Deserialize)]
struct DashReqCounts {
    request: RequestCount,
    response: ResponseCount,
}

#[derive(Deserialize)]
struct RequestCount {
    draft: u32,
}

#[derive(Deserialize)]
struct ResponseCount {
    new: u32,
}

#[derive(Deserialize)]
struct ReqResp {
    #[serde(rename = "requisitionCounts")]
    requisition_counts: FullReqCounts,
}

#[derive(Deserialize)]
struct FullReqCounts {
    request: RequestCount,
    response: ResponseCount,
    emergency: ResponseCount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct NamesResp {
    names: Connector,
}

#[derive(Deserialize)]
struct MasterListsResp {
    #[serde(rename = "masterLists")]
    master_lists: Connector,
}

pub async fn get_dashboard_summary(
    client: &OmSupplyClient,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: DashboardResp = client
        .query(DASHBOARD_QUERY, json!({ "storeId": resolved_store_id }))
        .await?;

    let items = &data.item_counts.item_counts;
    let stock = &data.stock_counts;
    let outbound = &data.outbound_shipment_counts;
    let inbound = &data.inbound_shipment_counts;
    let reqs = &data.requisition_counts;

    Ok(format!(
        "=== Open mSupply Dashboard ===\n\n\
         INVENTORY:\n  Total items: {}\n  No stock: {}\n  Low stock (<3 months): {}\n  Overstocked (>6 months): {}\n  Expired batches: {}\n  Expiring within 30 days: {}\n\n\
         OUTBOUND SHIPMENTS:\n  Created today: {}\n  Created this week: {}\n  Awaiting shipment: {}\n\n\
         INBOUND SHIPMENTS:\n  Created today: {}\n  Created this week: {}\n  Awaiting delivery: {}\n\n\
         REQUISITIONS:\n  Draft requests: {}\n  New responses to process: {}",
        items.total,
        items.no_stock,
        items.low_stock,
        items.high_stock,
        stock.expired,
        stock.expiring_soon,
        outbound.created.today,
        outbound.created.this_week,
        outbound.not_shipped,
        inbound.created.today,
        inbound.created.this_week,
        inbound.not_delivered,
        reqs.request.draft,
        reqs.response.new,
    ))
}

pub async fn get_requisition_counts(
    client: &OmSupplyClient,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let data: ReqResp = client
        .query(
            REQUISITION_COUNTS_QUERY,
            json!({ "storeId": resolved_store_id }),
        )
        .await?;
    let c = data.requisition_counts;
    Ok(format!(
        "Requisition Counts:\n  Draft requests: {}\n  New responses: {}\n  Emergency (new): {}",
        c.request.draft, c.response.new, c.emergency.new
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn search_names(
    client: &OmSupplyClient,
    search: Option<String>,
    code: Option<String>,
    is_supplier: Option<bool>,
    is_customer: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(s) = search {
        filter.insert("name".into(), json!({ "like": s }));
    }
    if let Some(c) = code {
        filter.insert("code".into(), json!({ "equalTo": c }));
    }
    if let Some(v) = is_supplier {
        filter.insert("isSupplier".into(), Value::Bool(v));
    }
    if let Some(v) = is_customer {
        filter.insert("isCustomer".into(), Value::Bool(v));
    }

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: NamesResp = client
        .query(
            NAMES_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "key": "name",
                "desc": false,
                "filter": filter_value,
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "names",
        &data.names.nodes,
        data.names.total_count,
        first,
        offset,
    ))
}

pub async fn get_master_lists(
    client: &OmSupplyClient,
    search: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(s) = search {
        filter.insert("name".into(), json!({ "like": s }));
    }
    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: MasterListsResp = client
        .query(
            MASTER_LISTS_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "filter": filter_value,
                "storeId": resolved_store_id,
                "sort": Value::Null,
            }),
        )
        .await?;

    Ok(format_list_result(
        "master lists",
        &data.master_lists.nodes,
        data.master_lists.total_count,
        first,
        offset,
    ))
}
