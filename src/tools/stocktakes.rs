//! Stocktake tools — physical inventory counts.
//!
//! Status flow: NEW -> FINALISED (only one transition; not reversible).
//! Only `stocktakeDate` (NaiveDate) is settable via update — `createdDatetime`
//! and `finalisedDatetime` are server-stamped at now() during their respective
//! operations. To "backdate" a stocktake, finalise it then set stocktakeDate
//! to the historic date.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::format_record;
use serde_json::{Value, json};
use uuid::Uuid;

const INSERT_MUTATION: &str = r#"
  mutation insertStocktake(
    $storeId: String!
    $input: InsertStocktakeInput!
  ) {
    insertStocktake(storeId: $storeId, input: $input) {
      __typename
      ... on StocktakeNode {
        id stocktakeNumber status description comment createdDatetime stocktakeDate
        isInitialStocktake isLocked
      }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updateStocktake(
    $storeId: String!
    $input: UpdateStocktakeInput!
  ) {
    updateStocktake(storeId: $storeId, input: $input) {
      __typename
      ... on StocktakeNode {
        id stocktakeNumber status description comment
        createdDatetime stocktakeDate finalisedDatetime
        isLocked countedBy verifiedBy
      }
      ... on UpdateStocktakeError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deleteStocktake(
    $storeId: String!
    $input: DeleteStocktakeInput!
  ) {
    deleteStocktake(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteStocktakeError {
        error { __typename description }
      }
    }
  }
"#;

const INSERT_LINE_MUTATION: &str = r#"
  mutation insertStocktakeLine(
    $storeId: String!
    $input: InsertStocktakeLineInput!
  ) {
    insertStocktakeLine(storeId: $storeId, input: $input) {
      __typename
      ... on StocktakeLineNode {
        id itemId batch expiryDate packSize countedNumberOfPacks
        snapshotNumberOfPacks costPricePerPack sellPricePerPack
      }
      ... on InsertStocktakeLineError {
        error { __typename description }
      }
    }
  }
"#;

const UPDATE_LINE_MUTATION: &str = r#"
  mutation updateStocktakeLine(
    $storeId: String!
    $input: UpdateStocktakeLineInput!
  ) {
    updateStocktakeLine(storeId: $storeId, input: $input) {
      __typename
      ... on StocktakeLineNode {
        id itemId batch expiryDate packSize countedNumberOfPacks
        snapshotNumberOfPacks costPricePerPack sellPricePerPack
      }
      ... on UpdateStocktakeLineError {
        error { __typename description }
      }
    }
  }
"#;

const DELETE_LINE_MUTATION: &str = r#"
  mutation deleteStocktakeLine(
    $storeId: String!
    $input: DeleteStocktakeLineInput!
  ) {
    deleteStocktakeLine(storeId: $storeId, input: $input) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteStocktakeLineError {
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
pub async fn insert_stocktake(
    client: &OmSupplyClient,
    is_all_items_stocktake: Option<bool>,
    master_list_id: Option<String>,
    include_all_master_list_items: Option<bool>,
    location_id: Option<String>,
    vvm_status_id: Option<String>,
    expires_before: Option<String>,
    is_initial_stocktake: Option<bool>,
    create_blank_stocktake: Option<bool>,
    description: Option<String>,
    comment: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({ "id": id });
    if let Some(v) = is_all_items_stocktake {
        input["isAllItemsStocktake"] = json!(v);
    }
    if let Some(v) = master_list_id {
        input["masterListId"] = json!(v);
    }
    if let Some(v) = include_all_master_list_items {
        input["includeAllMasterListItems"] = json!(v);
    }
    if let Some(v) = location_id {
        input["locationId"] = json!(v);
    }
    if let Some(v) = vvm_status_id {
        input["vvmStatusId"] = json!(v);
    }
    if let Some(v) = expires_before {
        input["expiresBefore"] = json!(v);
    }
    if let Some(v) = is_initial_stocktake {
        input["isInitialStocktake"] = json!(v);
    }
    if let Some(v) = create_blank_stocktake {
        input["createBlankStocktake"] = json!(v);
    }
    if let Some(v) = description {
        input["description"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("insertStocktake")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertStocktake".into()))?;
    let node = unwrap_mutation_response(response, "StocktakeNode")?;
    Ok(format!("Stocktake created:\n{}", format_record(&node)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_stocktake(
    client: &OmSupplyClient,
    id: String,
    status: Option<String>,
    stocktake_date: Option<String>,
    description: Option<String>,
    comment: Option<String>,
    is_locked: Option<bool>,
    counted_by: Option<String>,
    verified_by: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = status {
        input["status"] = json!(v);
    }
    if let Some(v) = stocktake_date {
        input["stocktakeDate"] = json!(v);
    }
    if let Some(v) = description {
        input["description"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }
    if let Some(v) = is_locked {
        input["isLocked"] = json!(v);
    }
    if let Some(v) = counted_by {
        input["countedBy"] = json!(v);
    }
    if let Some(v) = verified_by {
        input["verifiedBy"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateStocktake")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateStocktake".into()))?;
    let node = unwrap_mutation_response(response, "StocktakeNode")?;
    Ok(format!("Stocktake updated:\n{}", format_record(&node)))
}

pub async fn delete_stocktake(
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
        .get("deleteStocktake")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteStocktake".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Stocktake deleted (id={id})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_stocktake_line(
    client: &OmSupplyClient,
    stocktake_id: String,
    stock_line_id: Option<String>,
    item_id: Option<String>,
    counted_number_of_packs: Option<f64>,
    batch: Option<String>,
    expiry_date: Option<String>,
    manufacture_date: Option<String>,
    pack_size: Option<f64>,
    cost_price_per_pack: Option<f64>,
    sell_price_per_pack: Option<f64>,
    location_id: Option<String>,
    note: Option<String>,
    reason_option_id: Option<String>,
    item_variant_id: Option<String>,
    donor_id: Option<String>,
    manufacturer_id: Option<String>,
    vvm_status_id: Option<String>,
    campaign_id: Option<String>,
    program_id: Option<String>,
    comment: Option<String>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut input = json!({ "id": id, "stocktakeId": stocktake_id });
    if let Some(v) = stock_line_id {
        input["stockLineId"] = json!(v);
    }
    if let Some(v) = item_id {
        input["itemId"] = json!(v);
    }
    if let Some(v) = counted_number_of_packs {
        input["countedNumberOfPacks"] = json!(v);
    }
    if let Some(v) = batch {
        input["batch"] = json!(v);
    }
    if let Some(v) = expiry_date {
        input["expiryDate"] = json!(v);
    }
    if let Some(v) = manufacture_date {
        input["manufactureDate"] = json!(v);
    }
    if let Some(v) = pack_size {
        input["packSize"] = json!(v);
    }
    if let Some(v) = cost_price_per_pack {
        input["costPricePerPack"] = json!(v);
    }
    if let Some(v) = sell_price_per_pack {
        input["sellPricePerPack"] = json!(v);
    }
    if let Some(v) = location_id {
        input["location"] = json!({ "value": v });
    }
    if let Some(v) = note {
        input["note"] = json!(v);
    }
    if let Some(v) = reason_option_id {
        input["reasonOptionId"] = json!(v);
    }
    if let Some(v) = item_variant_id {
        input["itemVariantId"] = json!(v);
    }
    if let Some(v) = donor_id {
        input["donorId"] = json!(v);
    }
    if let Some(v) = manufacturer_id {
        input["manufacturerId"] = json!(v);
    }
    if let Some(v) = vvm_status_id {
        input["vvmStatusId"] = json!(v);
    }
    if let Some(v) = campaign_id {
        input["campaignId"] = json!(v);
    }
    if let Some(v) = program_id {
        input["programId"] = json!(v);
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
        .get("insertStocktakeLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertStocktakeLine".into()))?;
    let node = unwrap_mutation_response(response, "StocktakeLineNode")?;
    Ok(format!(
        "Stocktake line created:\n{}",
        format_record(&node)
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_stocktake_line(
    client: &OmSupplyClient,
    id: String,
    counted_number_of_packs: Option<f64>,
    snapshot_number_of_packs: Option<f64>,
    batch: Option<String>,
    expiry_date: Option<String>,
    manufacture_date: Option<String>,
    pack_size: Option<f64>,
    cost_price_per_pack: Option<f64>,
    sell_price_per_pack: Option<f64>,
    location_id: Option<String>,
    note: Option<String>,
    reason_option_id: Option<String>,
    comment: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = counted_number_of_packs {
        input["countedNumberOfPacks"] = json!(v);
    }
    if let Some(v) = snapshot_number_of_packs {
        input["snapshotNumberOfPacks"] = json!(v);
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
    if let Some(v) = pack_size {
        input["packSize"] = json!(v);
    }
    if let Some(v) = cost_price_per_pack {
        input["costPricePerPack"] = json!(v);
    }
    if let Some(v) = sell_price_per_pack {
        input["sellPricePerPack"] = json!(v);
    }
    if let Some(v) = location_id {
        input["location"] = json!({ "value": v });
    }
    if let Some(v) = note {
        input["note"] = json!(v);
    }
    if let Some(v) = reason_option_id {
        input["reasonOptionId"] = json!(v);
    }
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_LINE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;

    let response = data
        .get("updateStocktakeLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateStocktakeLine".into()))?;
    let node = unwrap_mutation_response(response, "StocktakeLineNode")?;
    Ok(format!(
        "Stocktake line updated:\n{}",
        format_record(&node)
    ))
}

pub async fn delete_stocktake_line(
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
        .get("deleteStocktakeLine")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteStocktakeLine".into()))?;
    unwrap_mutation_response(response, "DeleteResponse")?;
    Ok(format!("Stocktake line deleted (id={id})"))
}
