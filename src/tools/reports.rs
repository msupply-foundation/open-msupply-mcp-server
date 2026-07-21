//! Report tools — list reports, inspect their argument schema, and run them.
//!
//! Open mSupply reports are generated server-side. `generateReport` renders a
//! report to a file (PDF/HTML/Excel) and returns a `fileId`; the file is then
//! served from `{server}/files?id={fileId}`. For HTML output this module can
//! fetch and inline the rendered content so a caller can read it directly.
//!
//! Many reports are scoped to a record: pass `dataId` (e.g. an invoice id for an
//! outbound-shipment report, a requisition id, etc.) matching the report's
//! `context`. Parameterised reports take `arguments` (a JSON object) that must
//! satisfy the report's `argumentSchema.jsonSchema` — inspect it first with
//! `get_report_parameters`.

use crate::client::OmSupplyClient;
use crate::error::AppError;
use crate::format::{format_list_result, pagination_vars};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Rendered HTML larger than this is truncated in the inline result.
const MAX_INLINE_CHARS: usize = 40_000;

const REPORTS_QUERY: &str = r#"
  query reports(
    $storeId: String!
    $userLanguage: String!
    $filter: ReportFilterInput
    $sort: [ReportSortInput!]
  ) {
    reports(storeId: $storeId, userLanguage: $userLanguage, filter: $filter, sort: $sort) {
      ... on ReportConnector {
        __typename
        totalCount
        nodes {
          id name code context subContext isCustom isActive version
        }
      }
    }
  }
"#;

const REPORT_ARGS_QUERY: &str = r#"
  query reportArgs($storeId: String!, $userLanguage: String!, $filter: ReportFilterInput) {
    reports(storeId: $storeId, userLanguage: $userLanguage, filter: $filter) {
      ... on ReportConnector {
        __typename
        totalCount
        nodes {
          id name code context subContext
          argumentSchema { id type jsonSchema uiSchema }
        }
      }
    }
  }
"#;

const GENERATE_REPORT_QUERY: &str = r#"
  query generateReport(
    $storeId: String!
    $reportId: String!
    $dataId: String
    $arguments: JSON
    $format: PrintFormat
    $currentLanguage: String
  ) {
    generateReport(
      storeId: $storeId
      reportId: $reportId
      dataId: $dataId
      arguments: $arguments
      format: $format
      currentLanguage: $currentLanguage
    ) {
      __typename
      ... on PrintReportNode { fileId }
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
struct ReportsResp {
    reports: Connector,
}

fn build_filter(
    id: Option<String>,
    name: Option<String>,
    code: Option<String>,
    context: Option<String>,
) -> Value {
    let mut filter = Map::new();
    if let Some(v) = id {
        filter.insert("id".into(), json!({ "equalTo": v }));
    }
    if let Some(v) = name {
        filter.insert("name".into(), json!({ "like": v }));
    }
    if let Some(v) = code {
        filter.insert("code".into(), json!({ "equalTo": v }));
    }
    if let Some(v) = context {
        filter.insert("context".into(), json!({ "equalTo": v }));
    }
    if filter.is_empty() {
        Value::Null
    } else {
        Value::Object(filter)
    }
}

pub async fn list_reports(
    client: &OmSupplyClient,
    name: Option<String>,
    context: Option<String>,
    user_language: Option<String>,
    first: Option<u32>,
    offset: Option<u32>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let (first, offset) = pagination_vars(first, offset);
    let lang = user_language.unwrap_or_else(|| "en".into());
    let filter = build_filter(None, name, None, context);

    let data: ReportsResp = client
        .query(
            REPORTS_QUERY,
            json!({
                "storeId": resolved_store_id,
                "userLanguage": lang,
                "filter": filter,
                "sort": Value::Null,
            }),
        )
        .await?;

    // The server returns all matching reports; paginate client-side for a
    // consistent, bounded result (the reports query has no page argument).
    let total = data.reports.total_count;
    let page: Vec<Value> = data
        .reports
        .nodes
        .into_iter()
        .skip(offset as usize)
        .take(first as usize)
        .collect();

    Ok(format_list_result("reports", &page, total, first, offset))
}

pub async fn get_report_parameters(
    client: &OmSupplyClient,
    report_id: String,
    user_language: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let lang = user_language.unwrap_or_else(|| "en".into());
    let filter = build_filter(Some(report_id.clone()), None, None, None);

    let data: ReportsResp = client
        .query(
            REPORT_ARGS_QUERY,
            json!({
                "storeId": resolved_store_id,
                "userLanguage": lang,
                "filter": filter,
            }),
        )
        .await?;

    let report = data
        .reports
        .nodes
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Graphql(format!("Report not found (id={report_id})")))?;

    let name = report.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let context = report.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let sub_context = report.get("subContext").and_then(|v| v.as_str());
    let schema = report.pointer("/argumentSchema/jsonSchema");
    let ui_schema = report.pointer("/argumentSchema/uiSchema");

    let mut out = vec![
        format!("Report: {name} (id={report_id})"),
        format!("Context: {context}"),
    ];
    if let Some(sc) = sub_context {
        out.push(format!("SubContext: {sc}"));
    }
    out.push(String::new());
    match schema {
        Some(s) if !s.is_null() => {
            out.push("Argument JSON schema (pass matching fields as `arguments` to run_report):".to_string());
            out.push(serde_json::to_string_pretty(s).unwrap_or_else(|_| s.to_string()));
            if let Some(ui) = ui_schema {
                if !ui.is_null() {
                    out.push(String::new());
                    out.push("UI schema:".to_string());
                    out.push(serde_json::to_string_pretty(ui).unwrap_or_else(|_| ui.to_string()));
                }
            }
        }
        _ => out.push(
            "This report has no argument schema — it takes no parameters (it may still need a dataId for its context record).".to_string(),
        ),
    }
    Ok(out.join("\n"))
}

pub async fn run_report(
    client: &OmSupplyClient,
    report_id: String,
    data_id: Option<String>,
    arguments: Option<Value>,
    format: Option<String>,
    user_language: Option<String>,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    // Default to HTML so the rendered report can be inlined for the caller.
    let format = format
        .map(|f| f.trim().to_uppercase())
        .unwrap_or_else(|| "HTML".into());
    let lang = user_language.unwrap_or_else(|| "en".into());

    let data: Value = client
        .query(
            GENERATE_REPORT_QUERY,
            json!({
                "storeId": resolved_store_id,
                "reportId": report_id,
                "dataId": data_id,
                "arguments": arguments.unwrap_or(Value::Null),
                "format": format,
                "currentLanguage": lang,
            }),
        )
        .await?;

    let response = data
        .get("generateReport")
        .ok_or_else(|| AppError::UnexpectedResponse("missing generateReport".into()))?;
    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if typename != "PrintReportNode" {
        return Err(AppError::Graphql(format!(
            "report generation failed ({typename})"
        )));
    }
    let file_id = response
        .get("fileId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::UnexpectedResponse("missing fileId".into()))?;

    let url = client.file_url(file_id);
    let mut out = vec![
        "Report generated:".to_string(),
        format!("  reportId: {report_id}"),
        format!("  format: {format}"),
        format!("  fileId: {file_id}"),
        format!("  downloadUrl: {url}"),
    ];

    // For HTML, inline the rendered content (truncated) so it can be read
    // directly. PDF/Excel are binary — return the download URL only.
    if format == "HTML" {
        match client.fetch_file_text(file_id).await {
            Ok(content) => {
                out.push(String::new());
                if content.chars().count() > MAX_INLINE_CHARS {
                    let truncated: String = content.chars().take(MAX_INLINE_CHARS).collect();
                    out.push(format!(
                        "Rendered HTML (truncated to {MAX_INLINE_CHARS} chars — fetch downloadUrl for the full file):"
                    ));
                    out.push(truncated);
                } else {
                    out.push("Rendered HTML:".to_string());
                    out.push(content);
                }
            }
            Err(e) => {
                out.push(String::new());
                out.push(format!(
                    "(could not inline HTML content: {e}. Fetch it from downloadUrl.)"
                ));
            }
        }
    } else {
        out.push(String::new());
        out.push(format!(
            "Binary {format} file — download it from downloadUrl (authenticated request)."
        ));
    }

    Ok(out.join("\n"))
}
