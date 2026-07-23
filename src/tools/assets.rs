//! Cold-chain equipment (Asset) tools — the CCE registry (fridges, freezers,
//! cold boxes) classified class → category → type, linked to storage locations,
//! with a functional-status log. Plus read-only asset-catalogue discovery.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const ASSETS_QUERY: &str = r#"
  query assets(
    $storeId: String!
    $first: Int
    $offset: Int
    $filter: AssetFilterInput
  ) {
    assets(storeId: $storeId, page: { first: $first, offset: $offset }, filter: $filter) {
      ... on AssetConnector {
        __typename
        totalCount
        nodes {
          id assetNumber serialNumber storeId needsReplacement
          warrantyEnd replacementDate installationDate
          assetClass { id name } assetCategory { id name } assetType { id name }
          statusLog { status logDatetime }
          locations { nodes { id code name } }
        }
      }
    }
  }
"#;

const ASSET_DETAIL_QUERY: &str = r#"
  query assetDetail($storeId: String!, $filter: AssetFilterInput) {
    assets(storeId: $storeId, filter: $filter) {
      ... on AssetConnector {
        __typename
        totalCount
        nodes {
          id assetNumber serialNumber notes storeId catalogueItemId
          installationDate replacementDate warrantyStart warrantyEnd
          needsReplacement createdDatetime modifiedDatetime properties
          assetClass { id name } assetCategory { id name } assetType { id name }
          catalogueItem { id code }
          statusLog { status logDatetime comment }
          locations { totalCount nodes { id code name } }
        }
      }
    }
  }
"#;

const INSERT_MUTATION: &str = r#"
  mutation insertAsset($input: InsertAssetInput!, $storeId: String!) {
    insertAsset(input: $input, storeId: $storeId) {
      __typename
      ... on AssetNode { id assetNumber serialNumber }
      ... on InsertAssetError { error { __typename description } }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updateAsset($input: UpdateAssetInput!, $storeId: String!) {
    updateAsset(input: $input, storeId: $storeId) {
      __typename
      ... on AssetNode { id assetNumber serialNumber needsReplacement locations { nodes { id code } } }
      ... on UpdateAssetError { error { __typename description } }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deleteAsset($assetId: String!, $storeId: String!) {
    deleteAsset(assetId: $assetId, storeId: $storeId) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteAssetError { error { __typename description } }
    }
  }
"#;

const INSERT_LOG_MUTATION: &str = r#"
  mutation insertAssetLog($input: InsertAssetLogInput!, $storeId: String!) {
    insertAssetLog(input: $input, storeId: $storeId) {
      __typename
      ... on AssetLogNode { id status logDatetime }
      ... on InsertAssetLogError { error { __typename description } }
    }
  }
"#;

const ASSET_CLASSES_QUERY: &str = r#"
  query assetClasses($first: Int, $offset: Int) {
    assetClasses(page: { first: $first, offset: $offset }) {
      ... on AssetClassConnector { __typename totalCount nodes { id name } }
    }
  }
"#;

const ASSET_CATEGORIES_QUERY: &str = r#"
  query assetCategories($first: Int, $offset: Int, $filter: AssetCategoryFilterInput) {
    assetCategories(page: { first: $first, offset: $offset }, filter: $filter) {
      ... on AssetCategoryConnector { __typename totalCount nodes { id name classId } }
    }
  }
"#;

const ASSET_TYPES_QUERY: &str = r#"
  query assetTypes($first: Int, $offset: Int, $filter: AssetTypeFilterInput) {
    assetTypes(page: { first: $first, offset: $offset }, filter: $filter) {
      ... on AssetTypeConnector { __typename totalCount nodes { id name categoryId } }
    }
  }
"#;

const ASSET_CATALOGUE_ITEMS_QUERY: &str = r#"
  query assetCatalogueItems($first: Int, $offset: Int, $filter: AssetCatalogueItemFilterInput) {
    assetCatalogueItems(page: { first: $first, offset: $offset }, filter: $filter) {
      ... on AssetCatalogueItemConnector {
        __typename totalCount
        nodes { id code manufacturer model }
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

macro_rules! conn_resp {
    ($name:ident, $field:literal) => {
        #[derive(Deserialize)]
        struct $name {
            #[serde(rename = $field)]
            conn: Connector,
        }
    };
}
conn_resp!(AssetsResp, "assets");
conn_resp!(ClassesResp, "assetClasses");
conn_resp!(CategoriesResp, "assetCategories");
conn_resp!(TypesResp, "assetTypes");
conn_resp!(ItemsResp, "assetCatalogueItems");

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

#[allow(clippy::too_many_arguments)]
pub async fn list_assets(
    client: &OmSupplyClient,
    search: Option<String>,
    class_id: Option<String>,
    category_id: Option<String>,
    type_id: Option<String>,
    functioning_only: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = serde_json::Map::new();
    if let Some(s) = search {
        filter.insert("assetNumber".into(), json!({ "like": s }));
    }
    if let Some(v) = class_id {
        filter.insert("classId".into(), json!({ "equalTo": v }));
    }
    if let Some(v) = category_id {
        filter.insert("categoryId".into(), json!({ "equalTo": v }));
    }
    if let Some(v) = type_id {
        filter.insert("typeId".into(), json!({ "equalTo": v }));
    }
    if functioning_only == Some(true) {
        filter.insert("functionalStatus".into(), json!({ "equalTo": "FUNCTIONING" }));
    }
    let filter = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: AssetsResp = client
        .query(
            ASSETS_QUERY,
            json!({ "storeId": resolved_store_id, "first": first, "offset": offset, "filter": filter }),
        )
        .await?;
    Ok(format_list_result(
        "assets",
        &data.conn.nodes,
        data.conn.total_count,
        first,
        offset,
    ))
}

pub async fn get_asset(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let data: AssetsResp = client
        .query(
            ASSET_DETAIL_QUERY,
            json!({ "storeId": resolved_store_id, "filter": { "id": { "equalTo": id } } }),
        )
        .await?;
    let node = data
        .conn
        .nodes
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Graphql(format!("Asset not found (id={id})")))?;
    Ok(format!("Asset details:\n{}", format_record(&node)))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_asset(
    client: &OmSupplyClient,
    asset_number: Option<String>,
    serial_number: Option<String>,
    catalogue_item_id: Option<String>,
    class_id: Option<String>,
    category_id: Option<String>,
    type_id: Option<String>,
    installation_date: Option<String>,
    warranty_start: Option<String>,
    warranty_end: Option<String>,
    donor_name_id: Option<String>,
    notes: Option<String>,
    properties: Option<Value>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({ "id": id, "storeId": resolved_store_id });
    for (k, v) in [
        ("assetNumber", asset_number),
        ("serialNumber", serial_number),
        ("catalogueItemId", catalogue_item_id),
        ("classId", class_id),
        ("categoryId", category_id),
        ("typeId", type_id),
        ("installationDate", installation_date),
        ("warrantyStart", warranty_start),
        ("warrantyEnd", warranty_end),
        ("donorNameId", donor_name_id),
        ("notes", notes),
    ] {
        if let Some(val) = v {
            input[k] = json!(val);
        }
    }
    if let Some(p) = properties {
        input["properties"] = p;
    }

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("insertAsset")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertAsset".into()))?;
    let node = unwrap_mutation(response, "AssetNode")?;
    Ok(format!("Asset created:\n{}", format_record(&node)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_asset(
    client: &OmSupplyClient,
    id: String,
    asset_number: Option<String>,
    serial_number: Option<String>,
    catalogue_item_id: Option<String>,
    notes: Option<String>,
    installation_date: Option<String>,
    replacement_date: Option<String>,
    warranty_start: Option<String>,
    warranty_end: Option<String>,
    donor_name_id: Option<String>,
    needs_replacement: Option<bool>,
    location_ids: Option<Vec<String>>,
    properties: Option<Value>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    // Plain (non-nullable) fields.
    if let Some(v) = asset_number {
        input["assetNumber"] = json!(v);
    }
    if let Some(v) = notes {
        input["notes"] = json!(v);
    }
    if let Some(v) = needs_replacement {
        input["needsReplacement"] = json!(v);
    }
    if let Some(v) = location_ids {
        input["locationIds"] = json!(v);
    }
    if let Some(p) = properties {
        input["properties"] = p;
    }
    // Nullable (NullableUpdateInput) fields — wrap in { value: ... } to set.
    for (k, v) in [
        ("serialNumber", serial_number),
        ("catalogueItemId", catalogue_item_id),
        ("installationDate", installation_date),
        ("replacementDate", replacement_date),
        ("warrantyStart", warranty_start),
        ("warrantyEnd", warranty_end),
        ("donorNameId", donor_name_id),
    ] {
        if let Some(val) = v {
            input[k] = json!({ "value": val });
        }
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("updateAsset")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateAsset".into()))?;
    let node = unwrap_mutation(response, "AssetNode")?;
    Ok(format!("Asset updated:\n{}", format_record(&node)))
}

pub async fn delete_asset(
    client: &OmSupplyClient,
    id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let data: Value = client
        .query(
            DELETE_MUTATION,
            json!({ "storeId": resolved_store_id, "assetId": id }),
        )
        .await?;
    let response = data
        .get("deleteAsset")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteAsset".into()))?;
    unwrap_mutation(response, "DeleteResponse")?;
    Ok(format!("Asset deleted (id={id})"))
}

pub async fn set_asset_status(
    client: &OmSupplyClient,
    asset_id: String,
    status: String,
    reason_id: Option<String>,
    comment: Option<String>,
    log_datetime: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = Uuid::new_v4().to_string();

    let mut input = json!({ "id": id, "assetId": asset_id, "status": status });
    if let Some(v) = reason_id {
        input["reasonId"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = log_datetime {
        input["logDatetime"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_LOG_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("insertAssetLog")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertAssetLog".into()))?;
    let node = unwrap_mutation(response, "AssetLogNode")?;
    Ok(format!("Asset status recorded:\n{}", format_record(&node)))
}

// -------- Catalogue discovery (read-only; no store context) --------

pub async fn list_asset_classes(
    client: &OmSupplyClient,
    first: Option<u32>,
    offset: Option<u32>,
) -> Result<String, AppError> {
    let (first, offset) = pagination_vars(first, offset);
    let data: ClassesResp = client
        .query(ASSET_CLASSES_QUERY, json!({ "first": first, "offset": offset }))
        .await?;
    Ok(format_list_result(
        "asset classes",
        &data.conn.nodes,
        data.conn.total_count,
        first,
        offset,
    ))
}

pub async fn list_asset_categories(
    client: &OmSupplyClient,
    class_id: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
) -> Result<String, AppError> {
    let (first, offset) = pagination_vars(first, offset);
    let filter = match class_id {
        Some(v) => json!({ "classId": { "equalTo": v } }),
        None => Value::Null,
    };
    let data: CategoriesResp = client
        .query(
            ASSET_CATEGORIES_QUERY,
            json!({ "first": first, "offset": offset, "filter": filter }),
        )
        .await?;
    Ok(format_list_result(
        "asset categories",
        &data.conn.nodes,
        data.conn.total_count,
        first,
        offset,
    ))
}

pub async fn list_asset_types(
    client: &OmSupplyClient,
    category_id: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
) -> Result<String, AppError> {
    let (first, offset) = pagination_vars(first, offset);
    let filter = match category_id {
        Some(v) => json!({ "categoryId": { "equalTo": v } }),
        None => Value::Null,
    };
    let data: TypesResp = client
        .query(
            ASSET_TYPES_QUERY,
            json!({ "first": first, "offset": offset, "filter": filter }),
        )
        .await?;
    Ok(format_list_result(
        "asset types",
        &data.conn.nodes,
        data.conn.total_count,
        first,
        offset,
    ))
}

pub async fn list_asset_catalogue_items(
    client: &OmSupplyClient,
    search: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
) -> Result<String, AppError> {
    let (first, offset) = pagination_vars(first, offset);
    let filter = match search {
        Some(v) => json!({ "code": { "like": v } }),
        None => Value::Null,
    };
    let data: ItemsResp = client
        .query(
            ASSET_CATALOGUE_ITEMS_QUERY,
            json!({ "first": first, "offset": offset, "filter": filter }),
        )
        .await?;
    Ok(format_list_result(
        "asset catalogue items",
        &data.conn.nodes,
        data.conn.total_count,
        first,
        offset,
    ))
}
