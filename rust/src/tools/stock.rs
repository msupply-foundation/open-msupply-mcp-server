//! Stock tools -- port of src/tools/stock.ts

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const STOCK_LINES_QUERY: &str = r#"
  query stockLines(
    $first: Int
    $offset: Int
    $key: StockLineSortFieldInput!
    $desc: Boolean
    $filter: StockLineFilterInput
    $storeId: String!
  ) {
    stockLines(
      storeId: $storeId
      filter: $filter
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
    ) {
      ... on StockLineConnector {
        __typename
        totalCount
        nodes {
          id availableNumberOfPacks totalNumberOfPacks packSize batch
          expiryDate costPricePerPack sellPricePerPack locationName
          supplierName onHold itemId
          item { id code name unitName }
        }
      }
    }
  }
"#;

const STOCK_COUNTS_QUERY: &str = r#"
  query stockCounts($storeId: String!, $daysTillExpired: Int) {
    stockCounts(storeId: $storeId, daysTillExpired: $daysTillExpired) {
      expired
      expiringSoon
    }
  }
"#;

const ITEM_COUNTS_QUERY: &str = r#"
  query itemCounts(
    $storeId: String!
    $lowStockThreshold: Float!
    $highStockThreshold: Float!
  ) {
    itemCounts(
      storeId: $storeId
      lowStockThreshold: $lowStockThreshold
      highStockThreshold: $highStockThreshold
    ) {
      itemCounts { lowStock noStock highStock total }
    }
  }
"#;

const ITEM_LEDGER_QUERY: &str = r#"
  query itemLedger(
    $first: Int
    $offset: Int
    $filter: ItemLedgerFilterInput
    $storeId: String!
  ) {
    itemLedger(
      storeId: $storeId
      filter: $filter
      page: { first: $first, offset: $offset }
    ) {
      ... on ItemLedgerConnector {
        __typename
        totalCount
        nodes {
          id datetime invoiceNumber invoiceType invoiceStatus name
          batch expiryDate packSize numberOfPacks
          costPricePerPack sellPricePerPack reason
        }
      }
    }
  }
"#;

#[derive(Deserialize)]
struct StockLinesResp {
    #[serde(rename = "stockLines")]
    stock_lines: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct StockCountsResp {
    #[serde(rename = "stockCounts")]
    stock_counts: StockCounts,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StockCounts {
    expired: u32,
    expiring_soon: u32,
}

#[derive(Deserialize)]
struct ItemCountsResp {
    #[serde(rename = "itemCounts")]
    item_counts: ItemCountsOuter,
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
struct ItemLedgerResp {
    #[serde(rename = "itemLedger")]
    item_ledger: Connector,
}

#[allow(clippy::too_many_arguments)]
pub async fn get_stock_lines(
    client: &OmSupplyClient,
    item_id: Option<String>,
    search: Option<String>,
    location_id: Option<String>,
    has_stock: Option<bool>,
    sort_by: Option<String>,
    desc: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(id) = item_id {
        filter.insert("itemId".into(), json!({ "equalTo": id }));
    }
    if let Some(s) = search {
        filter.insert("itemCodeOrName".into(), json!({ "like": s }));
    }
    if let Some(loc) = location_id {
        filter.insert("locationId".into(), json!({ "equalTo": loc }));
    }
    if matches!(has_stock, Some(true)) {
        filter.insert("hasPacksInStore".into(), Value::Bool(true));
    }

    let key = match sort_by.as_deref() {
        Some(s @ ("expiryDate" | "itemName" | "itemCode" | "batch" | "numberOfPacks")) => s,
        _ => "itemName",
    };

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: StockLinesResp = client
        .query(
            STOCK_LINES_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "key": key,
                "desc": desc.unwrap_or(false),
                "filter": filter_value,
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "stock lines",
        &data.stock_lines.nodes,
        data.stock_lines.total_count,
        first,
        offset,
    ))
}

pub async fn get_stock_counts(
    client: &OmSupplyClient,
    days_till_expired: Option<i32>,
    low_stock_threshold: Option<f64>,
    high_stock_threshold: Option<f64>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let days = days_till_expired.unwrap_or(30);
    let low = low_stock_threshold.unwrap_or(3.0);
    let high = high_stock_threshold.unwrap_or(6.0);

    let stock_fut = client.query::<StockCountsResp>(
        STOCK_COUNTS_QUERY,
        json!({ "storeId": resolved_store_id, "daysTillExpired": days }),
    );
    let item_fut = client.query::<ItemCountsResp>(
        ITEM_COUNTS_QUERY,
        json!({
            "storeId": resolved_store_id,
            "lowStockThreshold": low,
            "highStockThreshold": high,
        }),
    );

    let (stock_data, item_data) = tokio::join!(stock_fut, item_fut);
    let stock = stock_data?.stock_counts;
    let counts = item_data?.item_counts.item_counts;

    Ok(format!(
        "Stock Summary:\n\n\
         Item Counts:\n  Total items: {total}\n  No stock: {no}\n  Low stock: {low_count}\n  High stock: {high_count}\n\n\
         Expiry:\n  Expired batches: {expired}\n  Expiring soon (within {days} days): {soon}",
        total = counts.total,
        no = counts.no_stock,
        low_count = counts.low_stock,
        high_count = counts.high_stock,
        expired = stock.expired,
        soon = stock.expiring_soon,
        days = days,
    ))
}

pub async fn get_item_ledger(
    client: &OmSupplyClient,
    item_id: String,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let data: ItemLedgerResp = client
        .query(
            ITEM_LEDGER_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "filter": { "itemId": { "equalTo": item_id } },
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "ledger entries",
        &data.item_ledger.nodes,
        data.item_ledger.total_count,
        first,
        offset,
    ))
}
