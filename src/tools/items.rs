//! Items tools -- port of src/tools/items.ts

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const ITEMS_QUERY: &str = r#"
  query items(
    $first: Int
    $offset: Int
    $key: ItemSortFieldInput!
    $desc: Boolean
    $filter: ItemFilterInput
    $storeId: String!
  ) {
    items(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
    ) {
      ... on ItemConnector {
        __typename
        totalCount
        nodes {
          id code name unitName isVaccine defaultPackSize type
          availableStockOnHand(storeId: $storeId)
          stats(storeId: $storeId) {
            averageMonthlyConsumption
            availableStockOnHand
            availableMonthsOfStockOnHand
          }
        }
      }
    }
  }
"#;

const ITEM_BY_ID_QUERY: &str = r#"
  query itemById($storeId: String!, $itemId: String!) {
    items(
      storeId: $storeId
      filter: { id: { equalTo: $itemId }, isActive: true }
    ) {
      ... on ItemConnector {
        __typename
        totalCount
        nodes {
          id code name unitName isVaccine defaultPackSize type
          strength doses volumePerPack weight
          availableStockOnHand(storeId: $storeId)
          stats(storeId: $storeId) {
            averageMonthlyConsumption
            availableStockOnHand
            availableMonthsOfStockOnHand
            totalConsumption
            stockOnHand
          }
          availableBatches(storeId: $storeId) {
            totalCount
            nodes {
              id batch expiryDate availableNumberOfPacks packSize
              costPricePerPack sellPricePerPack locationName supplierName onHold
            }
          }
        }
      }
    }
  }
"#;

#[derive(Deserialize)]
struct ItemsResp {
    items: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[allow(clippy::too_many_arguments)]
pub async fn search_items(
    client: &OmSupplyClient,
    search: Option<String>,
    code: Option<String>,
    is_vaccine: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    filter.insert("isActive".into(), Value::Bool(true));
    if let Some(s) = search {
        filter.insert("codeOrName".into(), json!({ "like": s }));
    }
    if let Some(c) = code {
        filter.insert("code".into(), json!({ "equalTo": c }));
    }
    if let Some(v) = is_vaccine {
        filter.insert("isVaccine".into(), Value::Bool(v));
    }

    let data: ItemsResp = client
        .query(
            ITEMS_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "key": "name",
                "desc": false,
                "filter": Value::Object(filter),
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "items",
        &data.items.nodes,
        data.items.total_count,
        first,
        offset,
    ))
}

pub async fn get_item(
    client: &OmSupplyClient,
    item_id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: ItemsResp = client
        .query(
            ITEM_BY_ID_QUERY,
            json!({ "storeId": resolved_store_id, "itemId": item_id }),
        )
        .await?;

    if data.items.total_count == 0 {
        return Err(AppError::Graphql(format!(
            "No item found with ID: {item_id}"
        )));
    }

    let item = &data.items.nodes[0];
    Ok(format!("Item details:\n{}", format_record(item)))
}
