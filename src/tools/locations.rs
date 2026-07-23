//! Location tools — storage-location CRUD, plus moving stock between locations.
//!
//! Locations were previously only readable as a `locationName` attribute on
//! stock lines, with no way to discover a `locationId`, toggle a location's
//! on-hold flag, or move stock. These wrap the server's `locations` query and
//! `insert/update/deleteLocation` mutations, plus `stock_relocation` for moving
//! a stock line to a different location (e.g. releasing a quarantined batch).

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const LOCATIONS_QUERY: &str = r#"
  query locations(
    $storeId: String!
    $first: Int
    $offset: Int
    $key: LocationSortFieldInput
    $desc: Boolean
    $filter: LocationFilterInput
  ) {
    locations(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      sort: { key: $key, desc: $desc }
      filter: $filter
    ) {
      ... on LocationConnector {
        __typename
        totalCount
        nodes {
          id code name onHold volume volumeUsed
          stock { totalCount }
        }
      }
    }
  }
"#;

const LOCATION_DETAIL_QUERY: &str = r#"
  query locationDetail($storeId: String!, $filter: LocationFilterInput) {
    locations(storeId: $storeId, filter: $filter) {
      ... on LocationConnector {
        __typename
        totalCount
        nodes {
          id code name onHold volume volumeUsed
          stock {
            totalCount
            nodes {
              id batch expiryDate packSize
              availableNumberOfPacks totalNumberOfPacks
              item { id code name }
            }
          }
        }
      }
    }
  }
"#;

const INSERT_MUTATION: &str = r#"
  mutation insertLocation($input: InsertLocationInput!, $storeId: String!) {
    insertLocation(input: $input, storeId: $storeId) {
      __typename
      ... on LocationNode { id code name onHold volume }
      ... on InsertLocationError { error { __typename description } }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updateLocation($input: UpdateLocationInput!, $storeId: String!) {
    updateLocation(input: $input, storeId: $storeId) {
      __typename
      ... on LocationNode { id code name onHold volume volumeUsed }
      ... on UpdateLocationError { error { __typename description } }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deleteLocation($input: DeleteLocationInput!, $storeId: String!) {
    deleteLocation(input: $input, storeId: $storeId) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteLocationError { error { __typename description } }
    }
  }
"#;

const STOCK_LINE_LOOKUP_QUERY: &str = r#"
  query stockLineLookup($storeId: String!, $filter: StockLineFilterInput) {
    stockLines(storeId: $storeId, filter: $filter) {
      ... on StockLineConnector {
        __typename
        totalCount
        nodes { id totalNumberOfPacks locationName }
      }
    }
  }
"#;

const INSERT_RELOCATION_MUTATION: &str = r#"
  mutation insertStockRelocation($storeId: String, $input: InsertStockRelocationInput!) {
    insertStockRelocation(storeId: $storeId, input: $input) {
      __typename
      ... on StockRelocationNode { id }
    }
  }
"#;

const BATCH_RELOCATION_LINE_MUTATION: &str = r#"
  mutation batchRelocation(
    $storeId: String
    $lineId: String!
    $relocationId: String!
    $stockLineId: String!
    $packs: Float!
    $locationId: String
  ) {
    batchStockRelocationLine(
      storeId: $storeId
      input: {
        upsert: [{
          id: $lineId
          stockRelocationId: $relocationId
          stockLineId: $stockLineId
          numberOfPacks: $packs
          destinationLocationId: $locationId
        }]
      }
    ) {
      upsert {
        id
        response {
          __typename
          ... on StockRelocationLineNode { id }
          ... on UpsertStockRelocationLineError { error { __typename description } }
        }
      }
    }
  }
"#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct LocationsResp {
    locations: Connector,
}

#[derive(Deserialize)]
struct StockLinesResp {
    #[serde(rename = "stockLines")]
    stock_lines: Connector,
}

/// Ok(node) if the response is the success type, else a readable error.
fn unwrap_mutation(response: &Value, success_typename: &str) -> Result<Value, AppError> {
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
        .unwrap_or("operation failed");
    Err(AppError::Graphql(format!("{err_typename}: {desc}")))
}

pub async fn list_locations(
    client: &OmSupplyClient,
    search: Option<String>,
    sort_by: Option<String>,
    desc: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let filter = match search {
        Some(s) => json!({ "name": { "like": s } }),
        None => Value::Null,
    };
    let key = sort_by.unwrap_or_else(|| "name".into());

    let data: LocationsResp = client
        .query(
            LOCATIONS_QUERY,
            json!({
                "storeId": resolved_store_id,
                "first": first,
                "offset": offset,
                "key": key,
                "desc": desc.unwrap_or(false),
                "filter": filter,
            }),
        )
        .await?;

    Ok(format_list_result(
        "locations",
        &data.locations.nodes,
        data.locations.total_count,
        first,
        offset,
    ))
}

pub async fn get_location(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: LocationsResp = client
        .query(
            LOCATION_DETAIL_QUERY,
            json!({
                "storeId": resolved_store_id,
                "filter": { "id": { "equalTo": id } },
            }),
        )
        .await?;

    let node = data
        .locations
        .nodes
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Graphql(format!("Location not found (id={id})")))?;
    Ok(format!("Location details:\n{}", format_record(&node)))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_location(
    client: &OmSupplyClient,
    code: String,
    name: Option<String>,
    on_hold: Option<bool>,
    location_type_id: Option<String>,
    volume: Option<f64>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({ "id": id, "code": code });
    if let Some(v) = name {
        input["name"] = json!(v);
    }
    if let Some(v) = on_hold {
        input["onHold"] = json!(v);
    }
    if let Some(v) = location_type_id {
        input["locationTypeId"] = json!(v);
    }
    if let Some(v) = volume {
        input["volume"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("insertLocation")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertLocation".into()))?;
    let node = unwrap_mutation(response, "LocationNode")?;
    Ok(format!("Location created:\n{}", format_record(&node)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_location(
    client: &OmSupplyClient,
    id: String,
    code: Option<String>,
    name: Option<String>,
    on_hold: Option<bool>,
    location_type_id: Option<String>,
    volume: Option<f64>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = code {
        input["code"] = json!(v);
    }
    if let Some(v) = name {
        input["name"] = json!(v);
    }
    if let Some(v) = on_hold {
        input["onHold"] = json!(v);
    }
    if let Some(v) = location_type_id {
        input["locationTypeId"] = json!(v);
    }
    if let Some(v) = volume {
        input["volume"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("updateLocation")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateLocation".into()))?;
    let node = unwrap_mutation(response, "LocationNode")?;
    Ok(format!("Location updated:\n{}", format_record(&node)))
}

pub async fn delete_location(
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
        .get("deleteLocation")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteLocation".into()))?;
    unwrap_mutation(response, "DeleteResponse")?;
    Ok(format!("Location deleted (id={id})"))
}

pub async fn relocate_stock(
    client: &OmSupplyClient,
    stock_line_id: String,
    to_location_id: String,
    number_of_packs: Option<f64>,
    comment: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    // Look up the stock line to learn its pack count (move all by default) and
    // its current location, for a useful result summary.
    let lookup: StockLinesResp = client
        .query(
            STOCK_LINE_LOOKUP_QUERY,
            json!({
                "storeId": resolved_store_id,
                "filter": { "id": { "equalTo": stock_line_id } },
            }),
        )
        .await?;
    let line = lookup
        .stock_lines
        .nodes
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Graphql(format!("Stock line not found (id={stock_line_id})")))?;
    let from_location = line
        .get("locationName")
        .and_then(|v| v.as_str())
        .unwrap_or("(none)")
        .to_string();
    let packs = match number_of_packs {
        Some(p) => p,
        None => line
            .get("totalNumberOfPacks")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                AppError::UnexpectedResponse("could not read stock line pack count".into())
            })?,
    };

    // 1. Create the relocation record.
    let relocation_id = Uuid::new_v4().to_string();
    let mut relocation_input = json!({ "id": relocation_id });
    if let Some(c) = comment {
        relocation_input["comment"] = json!(c);
    }
    let insert: Value = client
        .query(
            INSERT_RELOCATION_MUTATION,
            json!({ "storeId": resolved_store_id, "input": relocation_input }),
        )
        .await?;
    let insert_node = insert
        .get("insertStockRelocation")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertStockRelocation".into()))?;
    unwrap_mutation(insert_node, "StockRelocationNode")?;

    // 2. Move the stock line to the destination location.
    let line_id = Uuid::new_v4().to_string();
    let batch: Value = client
        .query(
            BATCH_RELOCATION_LINE_MUTATION,
            json!({
                "storeId": resolved_store_id,
                "lineId": line_id,
                "relocationId": relocation_id,
                "stockLineId": stock_line_id,
                "packs": packs,
                "locationId": to_location_id,
            }),
        )
        .await?;

    // Surface a per-line error from the batch result if present.
    if let Some(resp) = batch.pointer("/batchStockRelocationLine/upsert/0/response") {
        unwrap_mutation(resp, "StockRelocationLineNode")?;
    }

    Ok(format!(
        "Stock relocated:\n  relocationId: {relocation_id}\n  stockLineId: {stock_line_id}\n  packsMoved: {packs}\n  fromLocation: {from_location}\n  toLocationId: {to_location_id}"
    ))
}
