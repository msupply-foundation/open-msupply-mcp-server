//! Cold-chain monitoring tools — temperature sensors, temperature logs, and
//! temperature-breach (excursion) review/acknowledgement.
//!
//! Node field selections follow the spec's documented SensorNode /
//! TemperatureLogNode / TemperatureBreachNode fields. The two mutations select
//! only their success node (errors are reported generically) to stay robust.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, format_record, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const SENSORS_QUERY: &str = r#"
  query sensors($storeId: String!, $first: Int, $offset: Int) {
    sensors(storeId: $storeId, page: { first: $first, offset: $offset }) {
      ... on SensorConnector {
        __typename
        totalCount
        nodes {
          id name serial isActive batteryLevel logInterval lastConnectionDatetime
          location { id code name }
          breach
        }
      }
    }
  }
"#;

const TEMPERATURE_LOGS_QUERY: &str = r#"
  query temperatureLogs($storeId: String!, $first: Int, $offset: Int, $filter: TemperatureLogFilterInput) {
    temperatureLogs(storeId: $storeId, page: { first: $first, offset: $offset }, filter: $filter) {
      ... on TemperatureLogConnector {
        __typename
        totalCount
        nodes { id datetime temperature sensorId }
      }
    }
  }
"#;

const TEMPERATURE_BREACHES_QUERY: &str = r#"
  query temperatureBreaches($storeId: String!, $first: Int, $offset: Int, $filter: TemperatureBreachFilterInput) {
    temperatureBreaches(storeId: $storeId, page: { first: $first, offset: $offset }, filter: $filter) {
      ... on TemperatureBreachConnector {
        __typename
        totalCount
        nodes {
          id type startDatetime endDatetime durationMilliseconds
          sensorId unacknowledged thresholdTemperature
        }
      }
    }
  }
"#;

const UPDATE_SENSOR_MUTATION: &str = r#"
  mutation updateSensor($input: UpdateSensorInput!, $storeId: String!) {
    updateSensor(input: $input, storeId: $storeId) {
      __typename
      ... on SensorNode { id name isActive location { id code } }
    }
  }
"#;

const UPDATE_BREACH_MUTATION: &str = r#"
  mutation updateTemperatureBreach($input: UpdateTemperatureBreachInput!, $storeId: String!) {
    updateTemperatureBreach(input: $input, storeId: $storeId) {
      __typename
      ... on TemperatureBreachNode { id unacknowledged comment }
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
struct SensorsResp {
    sensors: Connector,
}
#[derive(Deserialize)]
struct LogsResp {
    #[serde(rename = "temperatureLogs")]
    temperature_logs: Connector,
}
#[derive(Deserialize)]
struct BreachesResp {
    #[serde(rename = "temperatureBreaches")]
    temperature_breaches: Connector,
}

fn unwrap_mutation(response: &Value, success_typename: &str) -> Result<Value, AppError> {
    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if typename == success_typename {
        return Ok(response.clone());
    }
    let desc = response
        .pointer("/error/description")
        .and_then(|v| v.as_str())
        .unwrap_or("operation failed");
    Err(AppError::Graphql(format!("{typename}: {desc}")))
}

pub async fn list_sensors(
    client: &OmSupplyClient,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);
    let data: SensorsResp = client
        .query(
            SENSORS_QUERY,
            json!({ "storeId": resolved_store_id, "first": first, "offset": offset }),
        )
        .await?;
    Ok(format_list_result(
        "sensors",
        &data.sensors.nodes,
        data.sensors.total_count,
        first,
        offset,
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn get_temperature_logs(
    client: &OmSupplyClient,
    sensor_id: Option<String>,
    from_datetime: Option<String>,
    to_datetime: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let mut filter = Map::new();
    if let Some(s) = sensor_id {
        filter.insert("sensor".into(), json!({ "id": { "equalTo": s } }));
    }
    let mut datetime = Map::new();
    if let Some(f) = from_datetime {
        datetime.insert("afterOrEqualTo".into(), json!(f));
    }
    if let Some(t) = to_datetime {
        datetime.insert("beforeOrEqualTo".into(), json!(t));
    }
    if !datetime.is_empty() {
        filter.insert("datetime".into(), Value::Object(datetime));
    }
    let filter = if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    };

    let data: LogsResp = client
        .query(
            TEMPERATURE_LOGS_QUERY,
            json!({ "storeId": resolved_store_id, "first": first, "offset": offset, "filter": filter }),
        )
        .await?;
    Ok(format_list_result(
        "temperature logs",
        &data.temperature_logs.nodes,
        data.temperature_logs.total_count,
        first,
        offset,
    ))
}

pub async fn list_temperature_breaches(
    client: &OmSupplyClient,
    unacknowledged_only: Option<bool>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);

    let filter = if unacknowledged_only == Some(true) {
        json!({ "unacknowledged": true })
    } else {
        Value::Null
    };

    let data: BreachesResp = client
        .query(
            TEMPERATURE_BREACHES_QUERY,
            json!({ "storeId": resolved_store_id, "first": first, "offset": offset, "filter": filter }),
        )
        .await?;
    Ok(format_list_result(
        "temperature breaches",
        &data.temperature_breaches.nodes,
        data.temperature_breaches.total_count,
        first,
        offset,
    ))
}

pub async fn update_sensor(
    client: &OmSupplyClient,
    id: String,
    name: Option<String>,
    is_active: Option<bool>,
    location_id: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id });
    if let Some(v) = name {
        input["name"] = json!(v);
    }
    if let Some(v) = is_active {
        input["isActive"] = json!(v);
    }
    if let Some(v) = location_id {
        // Nullable FK update (NullableUpdateInput pattern).
        input["location"] = json!({ "value": v });
    }

    let data: Value = client
        .query(
            UPDATE_SENSOR_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("updateSensor")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateSensor".into()))?;
    let node = unwrap_mutation(response, "SensorNode")?;
    Ok(format!("Sensor updated:\n{}", format_record(&node)))
}

pub async fn acknowledge_temperature_breach(
    client: &OmSupplyClient,
    id: String,
    comment: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    let mut input = json!({ "id": id, "unacknowledged": false });
    if let Some(v) = comment {
        input["comment"] = json!(v);
    }

    let data: Value = client
        .query(
            UPDATE_BREACH_MUTATION,
            json!({ "storeId": resolved_store_id, "input": input }),
        )
        .await?;
    let response = data
        .get("updateTemperatureBreach")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateTemperatureBreach".into()))?;
    let node = unwrap_mutation(response, "TemperatureBreachNode")?;
    Ok(format!("Temperature breach acknowledged:\n{}", format_record(&node)))
}
