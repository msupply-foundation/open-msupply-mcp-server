//! Store & system tools -- port of src/tools/stores.ts

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Value, json};

const STORES_QUERY: &str = r#"
  query stores($first: Int, $offset: Int) {
    stores(page: { first: $first, offset: $offset }, sort: { key: name }) {
      ... on StoreConnector {
        __typename
        totalCount
        nodes { id code storeName }
      }
    }
  }
"#;

const STORE_QUERY: &str = r#"
  query store($id: String!) {
    store(id: $id) {
      ... on StoreNode {
        __typename
        id
        code
        storeName
      }
      ... on NodeError {
        __typename
        error { description }
      }
    }
  }
"#;

const API_VERSION_QUERY: &str = r#"
  query apiVersion { apiVersion }
"#;

#[derive(Deserialize)]
struct ListResp {
    stores: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct StoreResp {
    store: Value,
}

#[derive(Deserialize)]
struct ApiVersionResp {
    #[serde(rename = "apiVersion")]
    api_version: String,
}

pub async fn list_stores(
    client: &OmSupplyClient,
    first: Option<u32>,
    offset: Option<u32>,
) -> Result<String, AppError> {
    let (first, offset) = pagination_vars(first, offset);
    let data: ListResp = client
        .query(STORES_QUERY, json!({ "first": first, "offset": offset }))
        .await?;
    Ok(format_list_result(
        "stores",
        &data.stores.nodes,
        data.stores.total_count,
        first,
        offset,
    ))
}

pub async fn get_store(client: &OmSupplyClient, id: String) -> Result<String, AppError> {
    let data: StoreResp = client.query(STORE_QUERY, json!({ "id": id })).await?;

    let typename = data
        .store
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if typename == "NodeError" {
        let desc = data
            .store
            .pointer("/error/description")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        return Err(AppError::Graphql(desc.to_string()));
    }

    Ok(format!("Store details:\n{}", format_record(&data.store)))
}

pub async fn get_server_info(client: &OmSupplyClient) -> Result<String, AppError> {
    let data: ApiVersionResp = client.query(API_VERSION_QUERY, json!({})).await?;
    let store_id = client
        .get_store_id()
        .await
        .unwrap_or_else(|| "(none - use list_stores to find one)".to_string());

    Ok(format!(
        "Open mSupply Server Info:\n  API Version: {}\n  Configured Store ID: {}",
        data.api_version, store_id
    ))
}
