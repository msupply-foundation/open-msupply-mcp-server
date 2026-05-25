//! Program / period / R&R-form-discovery helper tools.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const PROGRAMS_QUERY: &str = r#"
  query programs(
    $storeId: String!
    $first: Int
    $offset: Int
    $filter: ProgramFilterInput
    $sort: ProgramSortInput
  ) {
    programs(
      storeId: $storeId
      page: { first: $first, offset: $offset }
      filter: $filter
      sort: $sort
    ) {
      ... on ProgramConnector {
        __typename
        totalCount
        nodes { id name isImmunisation elmisCode }
      }
    }
  }
"#;

const PERIODS_QUERY: &str = r#"
  query periods(
    $storeId: String!
    $programId: String
    $first: Int
    $offset: Int
    $filter: PeriodFilterInput
  ) {
    periods(
      storeId: $storeId
      programId: $programId
      page: { first: $first, offset: $offset }
      filter: $filter
    ) {
      ... on PeriodConnector {
        __typename
        totalCount
        nodes { id name startDate endDate }
      }
    }
  }
"#;

const SUPPLIER_PROGRAM_SETTINGS_QUERY: &str = r#"
  query supplierProgramRequisitionSettings($storeId: String!) {
    supplierProgramRequisitionSettings(storeId: $storeId) {
      programName
      programId
      tagName
      masterList { id name code }
      suppliers { id name code isSupplier }
      orderTypes {
        id name isEmergency
        availablePeriods { id name startDate endDate }
      }
    }
  }
"#;

#[derive(Deserialize)]
struct ProgramsResp {
    programs: Connector,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connector {
    total_count: u32,
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
struct PeriodsResp {
    periods: Connector,
}

#[derive(Deserialize)]
struct SupplierSettingsResp {
    #[serde(rename = "supplierProgramRequisitionSettings")]
    settings: Vec<Value>,
}

pub async fn list_programs(
    client: &OmSupplyClient,
    search: Option<String>,
    is_immunisation: Option<bool>,
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
    if let Some(b) = is_immunisation {
        filter.insert("isImmunisation".into(), Value::Bool(b));
    }

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: ProgramsResp = client
        .query(
            PROGRAMS_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "filter": filter_value,
                "sort": { "key": "name", "desc": false },
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "programs",
        &data.programs.nodes,
        data.programs.total_count,
        first,
        offset,
    ))
}

pub async fn list_periods(
    client: &OmSupplyClient,
    program_id: Option<String>,
    start_date_after: Option<String>,
    end_date_before: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(d) = start_date_after {
        filter.insert("startDate".into(), json!({ "afterOrEqualTo": d }));
    }
    if let Some(d) = end_date_before {
        filter.insert("endDate".into(), json!({ "beforeOrEqualTo": d }));
    }

    let filter_value = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: PeriodsResp = client
        .query(
            PERIODS_QUERY,
            json!({
                "first": first,
                "offset": offset,
                "filter": filter_value,
                "programId": program_id,
                "storeId": resolved_store_id,
            }),
        )
        .await?;

    Ok(format_list_result(
        "periods",
        &data.periods.nodes,
        data.periods.total_count,
        first,
        offset,
    ))
}

pub async fn get_supplier_program_requisition_settings(
    client: &OmSupplyClient,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let data: SupplierSettingsResp = client
        .query(
            SUPPLIER_PROGRAM_SETTINGS_QUERY,
            json!({ "storeId": resolved_store_id }),
        )
        .await?;

    if data.settings.is_empty() {
        return Ok("No supplier program requisition settings found for this store.".into());
    }

    let mut out = vec![format!(
        "Found {} program(s) configured for this store:",
        data.settings.len()
    )];
    for s in &data.settings {
        out.push(String::new());
        out.push(format_record(s));
    }
    Ok(out.join("\n"))
}
