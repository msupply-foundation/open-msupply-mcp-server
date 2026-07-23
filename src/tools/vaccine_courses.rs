//! Vaccine course tools — CRUD for vaccine courses and their per-store config.
//!
//! A vaccine course belongs to a program and carries a list of vaccine items,
//! a dose schedule, and (nested) per-store config rows. There is no standalone
//! `vaccine_course_store_config` mutation on the server: store config is the
//! `storeConfigs` list on the insert/update vaccine-course input, and is read
//! back via `storeConfigs` on the course node. So this module CRUDs both the
//! course and its store config through the same course mutations.
//!
//! `insert`/`update` take the nested lists (`vaccineItems`, `doses`,
//! `storeConfigs`) as JSON arrays passed through verbatim — fetch the current
//! course with `get_vaccine_course`, modify, and send back (update replaces the
//! lists wholesale, like update_rnr_form).

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

const LIST_QUERY: &str = r#"
  query vaccineCourses($first: Int, $offset: Int, $filter: VaccineCourseFilterInput) {
    vaccineCourses(page: { first: $first, offset: $offset }, filter: $filter) {
      ... on VaccineCourseConnector {
        __typename
        totalCount
        nodes {
          id name programId demographicId
          coverageRate wastageRate useInGapsCalculations canSkipDose
        }
      }
    }
  }
"#;

const DETAIL_QUERY: &str = r#"
  query vaccineCourse($id: String!) {
    vaccineCourse(id: $id) {
      __typename
      ... on VaccineCourseNode {
        id name programId demographicId
        coverageRate wastageRate useInGapsCalculations canSkipDose
        vaccineCourseItems { id itemId }
        vaccineCourseDoses { id label minAge maxAge minIntervalDays customAgeLabel }
        storeConfigs { id storeId wastageRate coverageRate }
      }
      ... on NodeError { error { description } }
    }
  }
"#;

const INSERT_MUTATION: &str = r#"
  mutation insertVaccineCourse($input: InsertVaccineCourseInput!, $storeId: String!) {
    insertVaccineCourse(input: $input, storeId: $storeId) {
      __typename
      ... on VaccineCourseNode { id name programId }
      ... on InsertVaccineCourseError { error { __typename description } }
    }
  }
"#;

const UPDATE_MUTATION: &str = r#"
  mutation updateVaccineCourse($input: UpdateVaccineCourseInput!, $storeId: String!) {
    updateVaccineCourse(input: $input, storeId: $storeId) {
      __typename
      ... on VaccineCourseNode {
        id name
        vaccineCourseItems { id itemId }
        vaccineCourseDoses { id label }
        storeConfigs { id storeId wastageRate coverageRate }
      }
      ... on UpdateVaccineCourseError { error { __typename description } }
    }
  }
"#;

const DELETE_MUTATION: &str = r#"
  mutation deleteVaccineCourse($vaccineCourseId: String!) {
    deleteVaccineCourse(vaccineCourseId: $vaccineCourseId) {
      __typename
      ... on DeleteResponse { id }
      ... on DeleteVaccineCourseError { error { __typename description } }
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
struct ListResp {
    #[serde(rename = "vaccineCourses")]
    vaccine_courses: Connector,
}

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

pub async fn list_vaccine_courses(
    client: &OmSupplyClient,
    search: Option<String>,
    program_id: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
) -> Result<String, AppError> {
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(s) = search {
        filter.insert("name".into(), json!({ "like": s }));
    }
    if let Some(p) = program_id {
        filter.insert("programId".into(), json!({ "equalTo": p }));
    }
    let filter = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: ListResp = client
        .query(LIST_QUERY, json!({ "first": first, "offset": offset, "filter": filter }))
        .await?;
    Ok(format_list_result(
        "vaccine courses",
        &data.vaccine_courses.nodes,
        data.vaccine_courses.total_count,
        first,
        offset,
    ))
}

pub async fn get_vaccine_course(client: &OmSupplyClient, id: String) -> Result<String, AppError> {
    let data: Value = client.query(DETAIL_QUERY, json!({ "id": id })).await?;
    let node = data
        .get("vaccineCourse")
        .ok_or_else(|| AppError::UnexpectedResponse("missing vaccineCourse".into()))?;
    let typename = node.get("__typename").and_then(|v| v.as_str()).unwrap_or("");
    if typename != "VaccineCourseNode" {
        let desc = node
            .pointer("/error/description")
            .and_then(|v| v.as_str())
            .unwrap_or("vaccine course not found");
        return Err(AppError::Graphql(desc.to_string()));
    }
    Ok(format!("Vaccine course details:\n{}", format_record(node)))
}

/// Apply optional scalar rate/flag fields to a mutation input object.
fn apply_scalars(
    input: &mut Value,
    coverage_rate: Option<f64>,
    wastage_rate: Option<f64>,
    use_in_gaps_calculations: Option<bool>,
    can_skip_dose: Option<bool>,
    demographic_id: Option<String>,
) {
    if let Some(v) = coverage_rate {
        input["coverageRate"] = json!(v);
    }
    if let Some(v) = wastage_rate {
        input["wastageRate"] = json!(v);
    }
    if let Some(v) = use_in_gaps_calculations {
        input["useInGapsCalculations"] = json!(v);
    }
    if let Some(v) = can_skip_dose {
        input["canSkipDose"] = json!(v);
    }
    if let Some(v) = demographic_id {
        input["demographicId"] = json!(v);
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_vaccine_course(
    client: &OmSupplyClient,
    name: String,
    program_id: String,
    coverage_rate: Option<f64>,
    wastage_rate: Option<f64>,
    use_in_gaps_calculations: Option<bool>,
    can_skip_dose: Option<bool>,
    demographic_id: Option<String>,
    vaccine_items: Option<Value>,
    doses: Option<Value>,
    store_configs: Option<Value>,
    id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    // vaccineItems/doses are non-null lists server-side; default to empty.
    let mut input = json!({
        "id": id,
        "name": name,
        "programId": program_id,
        "vaccineItems": vaccine_items.unwrap_or_else(|| json!([])),
        "doses": doses.unwrap_or_else(|| json!([])),
    });
    apply_scalars(
        &mut input,
        coverage_rate,
        wastage_rate,
        use_in_gaps_calculations,
        can_skip_dose,
        demographic_id,
    );
    if let Some(sc) = store_configs {
        input["storeConfigs"] = sc;
    }

    let data: Value = client
        .query(
            INSERT_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("insertVaccineCourse")
        .ok_or_else(|| AppError::UnexpectedResponse("missing insertVaccineCourse".into()))?;
    let node = unwrap_mutation(response, "VaccineCourseNode")?;
    Ok(format!("Vaccine course created:\n{}", format_record(&node)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_vaccine_course(
    client: &OmSupplyClient,
    id: String,
    vaccine_items: Value,
    doses: Value,
    name: Option<String>,
    coverage_rate: Option<f64>,
    wastage_rate: Option<f64>,
    use_in_gaps_calculations: Option<bool>,
    can_skip_dose: Option<bool>,
    demographic_id: Option<String>,
    store_configs: Option<Value>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    // update replaces the lists wholesale — vaccineItems/doses are required.
    let mut input = json!({
        "id": id,
        "vaccineItems": vaccine_items,
        "doses": doses,
    });
    if let Some(v) = name {
        input["name"] = json!(v);
    }
    apply_scalars(
        &mut input,
        coverage_rate,
        wastage_rate,
        use_in_gaps_calculations,
        can_skip_dose,
        demographic_id,
    );
    if let Some(sc) = store_configs {
        input["storeConfigs"] = sc;
    }

    let data: Value = client
        .query(
            UPDATE_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("updateVaccineCourse")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateVaccineCourse".into()))?;
    let node = unwrap_mutation(response, "VaccineCourseNode")?;
    Ok(format!("Vaccine course updated:\n{}", format_record(&node)))
}

pub async fn delete_vaccine_course(
    client: &OmSupplyClient,
    id: String,
) -> Result<String, AppError> {
    let data: Value = client
        .query(DELETE_MUTATION, json!({ "vaccineCourseId": id }))
        .await?;
    let response = data
        .get("deleteVaccineCourse")
        .ok_or_else(|| AppError::UnexpectedResponse("missing deleteVaccineCourse".into()))?;
    unwrap_mutation(response, "DeleteResponse")?;
    Ok(format!("Vaccine course deleted (id={id})"))
}
