//! MCP tool router -- maps the 16 tools exposed by the TypeScript server onto rmcp.
//!
//! Each `#[tool]` method is a thin wrapper that:
//! 1. destructures typed parameters (via `Parameters<T>`),
//! 2. delegates to a free function in the relevant sub-module,
//! 3. converts `Result<String, AppError>` into the MCP `CallToolResult` envelope.

mod dashboard;
mod fulfil;
mod inbound_shipments;
mod invoices;
mod items;
mod outbound_shipments;
mod programs;
mod purchase_orders;
mod requisitions;
mod rnr;
mod stock;
mod stocktakes;
mod stores;

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::client::OmSupplyClient;
use crate::error::AppError;

/// Lenient deserializers for scalar params.
///
/// Some MCP client bridges (e.g. the desktop/MCPB stdio bridge) serialize *all*
/// tool-call arguments as JSON strings -- so `{"first": 25, "isVaccine": true}`
/// arrives as `{"first": "25", "isVaccine": "true"}`. Our params are strictly
/// typed, so serde would reject the string form. These helpers accept either the
/// native JSON type or its string encoding. The advertised JSON schema is
/// unchanged (still number/boolean), so well-behaved clients are unaffected.
///
/// Each optional field using one of these MUST also carry `#[serde(default)]`,
/// because `deserialize_with` disables serde's implicit "missing Option -> None".
mod flex {
    use std::fmt::Display;
    use std::str::FromStr;

    use serde::{Deserialize, Deserializer, de};
    use serde_json::Value;

    fn parse_bool(s: &str) -> Result<bool, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => Ok(true),
            "false" | "0" | "no" | "n" => Ok(false),
            other => Err(format!("invalid boolean string: {other:?}")),
        }
    }

    /// Accept a JSON bool, a stringified bool ("true"/"false"/"1"/"0"), or a number.
    pub fn opt_bool<'de, D>(d: D) -> Result<Option<bool>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<Value>::deserialize(d)? {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(b)),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => parse_bool(&s).map(Some).map_err(de::Error::custom),
            Some(Value::Number(n)) => Ok(Some(n.as_f64().map(|f| f != 0.0).unwrap_or(false))),
            Some(other) => Err(de::Error::custom(format!("expected boolean, got {other}"))),
        }
    }

    /// Accept a JSON number or a stringified number; `null`/missing/empty -> None.
    fn opt_num<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr + Deserialize<'de>,
        <T as FromStr>::Err: Display,
    {
        match Option::<Value>::deserialize(d)? {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => s.trim().parse::<T>().map(Some).map_err(de::Error::custom),
            Some(v) => T::deserialize(v).map(Some).map_err(de::Error::custom),
        }
    }

    /// Accept a JSON number or a stringified number; field must be present.
    fn req_num<'de, D, T>(d: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr + Deserialize<'de>,
        <T as FromStr>::Err: Display,
    {
        match Value::deserialize(d)? {
            Value::String(s) => s.trim().parse::<T>().map_err(de::Error::custom),
            v => T::deserialize(v).map_err(de::Error::custom),
        }
    }

    pub fn opt_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
        opt_num(d)
    }
    pub fn opt_i32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
        opt_num(d)
    }
    pub fn opt_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        opt_num(d)
    }
    pub fn req_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        req_num(d)
    }
}

#[derive(Clone)]
pub struct OmSupplyServer {
    client: Arc<OmSupplyClient>,
    #[allow(dead_code)]
    tool_router: ToolRouter<OmSupplyServer>,
}

impl OmSupplyServer {
    pub fn new(client: Arc<OmSupplyClient>) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }
}

// -------- Parameter structs --------
// rmcp requires a single `Parameters<T>` argument per tool, where T derives
// Deserialize + JsonSchema. We declare one struct per tool.

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListStoresParams {
    /// Max number of results to return (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Number of results to skip for pagination
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetStoreParams {
    /// The store ID
    pub id: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchItemsParams {
    /// Search term to match against item name or code
    pub search: Option<String>,
    /// Filter by exact item code
    pub code: Option<String>,
    /// Filter to only vaccine items
    #[serde(rename = "isVaccine", default, deserialize_with = "flex::opt_bool")]
    pub is_vaccine: Option<bool>,
    /// Max results to return (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Number of results to skip for pagination
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetItemParams {
    /// The item ID to look up
    #[serde(rename = "itemId")]
    pub item_id: String,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StoreIdParams {
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetStockLinesParams {
    /// Filter stock lines by item ID
    #[serde(rename = "itemId")]
    pub item_id: Option<String>,
    /// Search by item code or name
    pub search: Option<String>,
    /// Filter by location ID
    #[serde(rename = "locationId")]
    pub location_id: Option<String>,
    /// If true, only show lines with available packs
    #[serde(rename = "hasStock", default, deserialize_with = "flex::opt_bool")]
    pub has_stock: Option<bool>,
    /// Sort field: expiryDate | itemName | itemCode | batch | numberOfPacks (default: itemName)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    /// Sort descending
    #[serde(default, deserialize_with = "flex::opt_bool")]
    pub desc: Option<bool>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetStockCountsParams {
    /// Days-until-expired threshold (default 30)
    #[serde(rename = "daysTillExpired", default, deserialize_with = "flex::opt_i32")]
    pub days_till_expired: Option<i32>,
    /// Months-of-stock below which items are "low stock" (default 3)
    #[serde(rename = "lowStockThreshold", default, deserialize_with = "flex::opt_f64")]
    pub low_stock_threshold: Option<f64>,
    /// Months-of-stock above which items are "high stock" (default 6)
    #[serde(rename = "highStockThreshold", default, deserialize_with = "flex::opt_f64")]
    pub high_stock_threshold: Option<f64>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetItemLedgerParams {
    /// The item ID to get ledger for
    #[serde(rename = "itemId")]
    pub item_id: String,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListInvoicesParams {
    /// Filter by invoice type: OUTBOUND_SHIPMENT | INBOUND_SHIPMENT | PRESCRIPTION | SUPPLIER_RETURN | CUSTOMER_RETURN
    #[serde(rename = "type")]
    pub invoice_type: Option<String>,
    /// Filter by invoice status: NEW | ALLOCATED | PICKED | SHIPPED | DELIVERED | VERIFIED
    pub status: Option<String>,
    /// Filter by other party (supplier/customer) name
    #[serde(rename = "otherPartyName")]
    pub other_party_name: Option<String>,
    /// Sort field: invoiceNumber | otherPartyName | status | createdDatetime | type (default: createdDatetime)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    /// Sort descending (default true)
    #[serde(default, deserialize_with = "flex::opt_bool")]
    pub desc: Option<bool>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetInvoiceParams {
    /// The invoice ID
    pub id: String,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchNamesParams {
    /// Search term to match against name
    pub search: Option<String>,
    /// Filter by exact code
    pub code: Option<String>,
    /// Filter to only suppliers
    #[serde(rename = "isSupplier", default, deserialize_with = "flex::opt_bool")]
    pub is_supplier: Option<bool>,
    /// Filter to only customers
    #[serde(rename = "isCustomer", default, deserialize_with = "flex::opt_bool")]
    pub is_customer: Option<bool>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetMasterListsParams {
    /// Search term to match against master list name
    pub search: Option<String>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Requisition param structs --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRequisitionsParams {
    /// Filter by type: REQUEST | RESPONSE
    #[serde(rename = "type")]
    pub requisition_type: Option<String>,
    /// Filter by status: DRAFT | NEW | SENT | FINALISED
    pub status: Option<String>,
    /// Filter by other party (supplier/customer) name (substring match)
    #[serde(rename = "otherPartyName")]
    pub other_party_name: Option<String>,
    /// Filter by program ID
    #[serde(rename = "programId")]
    pub program_id: Option<String>,
    /// Filter to only emergency requisitions
    #[serde(rename = "isEmergency", default, deserialize_with = "flex::opt_bool")]
    pub is_emergency: Option<bool>,
    /// Sort field: requisitionNumber | type | status | otherPartyName | sentDatetime | createdDatetime | finalisedDatetime | expectedDeliveryDate | theirReference | orderType | programName | periodStartDate | comment (default: createdDatetime)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    /// Sort descending (default true)
    #[serde(default, deserialize_with = "flex::opt_bool")]
    pub desc: Option<bool>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRequisitionParams {
    /// The requisition ID
    pub id: String,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertRequestRequisitionParams {
    /// Supplier/other party ID (use search_names with isSupplier=true to find one)
    #[serde(rename = "otherPartyId")]
    pub other_party_id: String,
    /// Maximum months of stock to keep on hand (e.g. 6.0)
    #[serde(rename = "maxMonthsOfStock", deserialize_with = "flex::req_f64")]
    pub max_months_of_stock: f64,
    /// Minimum months of stock threshold (e.g. 3.0)
    #[serde(rename = "minMonthsOfStock", deserialize_with = "flex::req_f64")]
    pub min_months_of_stock: f64,
    /// Their (supplier-side) reference for this requisition
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    /// Free-form comment
    pub comment: Option<String>,
    /// Hex colour code (e.g. "#ff0000") for UI display
    pub colour: Option<String>,
    /// Expected delivery date in ISO format (YYYY-MM-DD)
    #[serde(rename = "expectedDeliveryDate")]
    pub expected_delivery_date: Option<String>,
    /// Optional client-supplied UUID. If omitted, a v4 UUID is generated.
    pub id: Option<String>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateRequestRequisitionParams {
    /// The requisition ID
    pub id: String,
    /// Status transition. Only "SENT" is accepted -- this is how you submit a request requisition.
    pub status: Option<String>,
    /// Free-form comment
    pub comment: Option<String>,
    /// Their (supplier-side) reference
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    /// Hex colour code
    pub colour: Option<String>,
    /// Change supplier
    #[serde(rename = "otherPartyId")]
    pub other_party_id: Option<String>,
    /// Expected delivery date (YYYY-MM-DD)
    #[serde(rename = "expectedDeliveryDate")]
    pub expected_delivery_date: Option<String>,
    /// Maximum months of stock
    #[serde(rename = "maxMonthsOfStock", default, deserialize_with = "flex::opt_f64")]
    pub max_months_of_stock: Option<f64>,
    /// Minimum months of stock
    #[serde(rename = "minMonthsOfStock", default, deserialize_with = "flex::opt_f64")]
    pub min_months_of_stock: Option<f64>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteByIdParams {
    /// The record ID to delete
    pub id: String,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertRequestRequisitionLineParams {
    /// The requisition ID this line belongs to
    #[serde(rename = "requisitionId")]
    pub requisition_id: String,
    /// The item ID to request (use search_items to find one)
    #[serde(rename = "itemId")]
    pub item_id: String,
    /// Requested quantity (units). If provided, line is updated immediately after insert.
    #[serde(rename = "requestedQuantity", default, deserialize_with = "flex::opt_f64")]
    pub requested_quantity: Option<f64>,
    /// Optional comment about this line
    pub comment: Option<String>,
    /// Optional client-supplied UUID for the line
    pub id: Option<String>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateRequestRequisitionLineParams {
    /// The requisition line ID
    pub id: String,
    /// Requested quantity
    #[serde(rename = "requestedQuantity", default, deserialize_with = "flex::opt_f64")]
    pub requested_quantity: Option<f64>,
    /// Comment
    pub comment: Option<String>,
    /// Reason / option ID
    #[serde(rename = "optionId")]
    pub option_id: Option<String>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- R&R form param structs --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRnrFormsParams {
    /// Filter by program ID
    #[serde(rename = "programId")]
    pub program_id: Option<String>,
    /// Filter by period schedule ID
    #[serde(rename = "periodScheduleId")]
    pub period_schedule_id: Option<String>,
    /// Sort field: period | program | createdDatetime | status | supplierName (default: createdDatetime)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    /// Sort descending (default true)
    #[serde(default, deserialize_with = "flex::opt_bool")]
    pub desc: Option<bool>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRnrFormParams {
    /// The R&R form ID
    #[serde(rename = "rnrFormId")]
    pub rnr_form_id: String,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertRnrFormParams {
    /// Supplier name ID (from get_supplier_program_requisition_settings)
    #[serde(rename = "supplierId")]
    pub supplier_id: String,
    /// Program ID (from get_supplier_program_requisition_settings or list_programs)
    #[serde(rename = "programId")]
    pub program_id: String,
    /// Period ID for this reporting period (from list_periods)
    #[serde(rename = "periodId")]
    pub period_id: String,
    /// Optional client-supplied UUID
    pub id: Option<String>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateRnrFormParams {
    /// The R&R form ID
    pub id: String,
    /// Full array of UpdateRnRFormLineInput objects -- this REPLACES all lines on the form.
    /// Each line must include: id, stockOutDuration, adjustedQuantityConsumed, averageMonthlyConsumption,
    /// initialBalance, finalBalance, minimumQuantity, maximumQuantity, calculatedRequestedQuantity,
    /// lowStock (BELOW_QUARTER | BELOW_HALF | OK), confirmed.
    /// Optional fields: quantityReceived, quantityConsumed, losses, adjustments, expiryDate (YYYY-MM-DD),
    /// enteredRequestedQuantity, comment.
    /// Tip: fetch the current lines with get_rnr_form, modify, then send back.
    pub lines: serde_json::Value,
    /// Their (supplier-side) reference
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    /// Free-form comment
    pub comment: Option<String>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Program / period discovery params --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListProgramsParams {
    /// Search term to match against program name
    pub search: Option<String>,
    /// Filter to only immunisation programs
    #[serde(rename = "isImmunisation", default, deserialize_with = "flex::opt_bool")]
    pub is_immunisation: Option<bool>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPeriodsParams {
    /// Limit periods to those tied to this program ID
    #[serde(rename = "programId")]
    pub program_id: Option<String>,
    /// Periods starting on or after this date (YYYY-MM-DD)
    #[serde(rename = "startDateAfter")]
    pub start_date_after: Option<String>,
    /// Periods ending on or before this date (YYYY-MM-DD)
    #[serde(rename = "endDateBefore")]
    pub end_date_before: Option<String>,
    /// Max results (default 25)
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    /// Pagination offset
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Outbound shipment params --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertOutboundShipmentParams {
    /// Customer ID (use search_names with isCustomer=true)
    #[serde(rename = "otherPartyId")]
    pub other_party_id: String,
    /// Place shipment on hold
    #[serde(rename = "onHold", default, deserialize_with = "flex::opt_bool")]
    pub on_hold: Option<bool>,
    pub comment: Option<String>,
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    pub colour: Option<String>,
    /// Optional client-supplied UUID
    pub id: Option<String>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateOutboundShipmentParams {
    /// The shipment ID
    pub id: String,
    /// Status transition: ALLOCATED | PICKED | SHIPPED (cannot reverse)
    pub status: Option<String>,
    #[serde(rename = "onHold", default, deserialize_with = "flex::opt_bool")]
    pub on_hold: Option<bool>,
    pub comment: Option<String>,
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    #[serde(rename = "transportReference")]
    pub transport_reference: Option<String>,
    pub colour: Option<String>,
    /// Expected delivery date (YYYY-MM-DD)
    #[serde(rename = "expectedDeliveryDate")]
    pub expected_delivery_date: Option<String>,
    /// ISO-8601 UTC datetime (e.g. "2026-01-15T10:00:00Z"). When supplied alongside
    /// a status transition, the server walks all earlier status timestamps back to
    /// this value -- creating a fully historic shipment in one call.
    #[serde(rename = "backdatedDatetime")]
    pub backdated_datetime: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertOutboundShipmentLineParams {
    /// Invoice (shipment) ID this line belongs to
    #[serde(rename = "invoiceId")]
    pub invoice_id: String,
    /// Stock line ID to issue from (use get_stock_lines to discover)
    #[serde(rename = "stockLineId")]
    pub stock_line_id: String,
    /// Number of packs to issue
    #[serde(rename = "numberOfPacks", deserialize_with = "flex::req_f64")]
    pub number_of_packs: f64,
    #[serde(rename = "taxPercentage", default, deserialize_with = "flex::opt_f64")]
    pub tax_percentage: Option<f64>,
    #[serde(rename = "vvmStatusId")]
    pub vvm_status_id: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateOutboundShipmentLineParams {
    pub id: String,
    #[serde(rename = "stockLineId")]
    pub stock_line_id: Option<String>,
    #[serde(rename = "numberOfPacks", default, deserialize_with = "flex::opt_f64")]
    pub number_of_packs: Option<f64>,
    #[serde(rename = "prescribedQuantity", default, deserialize_with = "flex::opt_f64")]
    pub prescribed_quantity: Option<f64>,
    #[serde(rename = "vvmStatusId")]
    pub vvm_status_id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Requisition fulfilment params --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SupplyRequestedQuantityParams {
    /// The response requisition ID to supply (type RESPONSE)
    #[serde(rename = "responseRequisitionId")]
    pub response_requisition_id: String,
    /// Supplying store ID (uses default if not provided). Must own the requisition.
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateRequisitionShipmentParams {
    /// The response requisition ID to fulfil (use list_requisitions/get_requisition, type RESPONSE)
    #[serde(rename = "responseRequisitionId")]
    pub response_requisition_id: String,
    /// Supplying store ID (uses default if not provided). Must own the requisition.
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AllocateOutboundShipmentLineParams {
    /// The unallocated outbound-shipment line ID to allocate to stock
    #[serde(rename = "lineId")]
    pub line_id: String,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FulfilRequisitionParams {
    /// The response requisition ID to fulfil (type RESPONSE)
    #[serde(rename = "responseRequisitionId")]
    pub response_requisition_id: String,
    /// If true, advance the created shipment to SHIPPED after allocating all lines (default false)
    #[serde(default, deserialize_with = "flex::opt_bool")]
    pub ship: Option<bool>,
    /// Supplying store ID (uses default if not provided). Must own the requisition.
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Inbound shipment params --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertInboundShipmentParams {
    /// Supplier ID (use search_names with isSupplier=true)
    #[serde(rename = "otherPartyId")]
    pub other_party_id: String,
    #[serde(rename = "onHold", default, deserialize_with = "flex::opt_bool")]
    pub on_hold: Option<bool>,
    pub comment: Option<String>,
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    pub colour: Option<String>,
    /// Optionally link to a requisition
    #[serde(rename = "requisitionId")]
    pub requisition_id: Option<String>,
    /// Optionally link to a purchase order
    #[serde(rename = "purchaseOrderId")]
    pub purchase_order_id: Option<String>,
    /// If true and purchaseOrderId set, pre-fill lines from the PO
    #[serde(rename = "insertLinesFromPurchaseOrder", default, deserialize_with = "flex::opt_bool")]
    pub insert_lines_from_purchase_order: Option<bool>,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateInboundShipmentParams {
    pub id: String,
    /// Status transition: SHIPPED | DELIVERED | RECEIVED | VERIFIED (cannot reverse).
    /// Only `receivedDatetime` is backdate-able; delivered/verified are server-stamped.
    pub status: Option<String>,
    #[serde(rename = "onHold", default, deserialize_with = "flex::opt_bool")]
    pub on_hold: Option<bool>,
    pub comment: Option<String>,
    #[serde(rename = "theirReference")]
    pub their_reference: Option<String>,
    pub colour: Option<String>,
    #[serde(rename = "otherPartyId")]
    pub other_party_id: Option<String>,
    /// ISO-8601 UTC datetime to backdate the received timestamp
    #[serde(rename = "receivedDatetime")]
    pub received_datetime: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertInboundShipmentLineParams {
    #[serde(rename = "invoiceId")]
    pub invoice_id: String,
    /// Item being received (use search_items)
    #[serde(rename = "itemId")]
    pub item_id: String,
    /// Pack size (units per pack)
    #[serde(rename = "packSize", deserialize_with = "flex::req_f64")]
    pub pack_size: f64,
    /// Number of packs being received
    #[serde(rename = "numberOfPacks", deserialize_with = "flex::req_f64")]
    pub number_of_packs: f64,
    #[serde(rename = "costPricePerPack", deserialize_with = "flex::req_f64")]
    pub cost_price_per_pack: f64,
    #[serde(rename = "sellPricePerPack", deserialize_with = "flex::req_f64")]
    pub sell_price_per_pack: f64,
    pub batch: Option<String>,
    /// Expiry date (YYYY-MM-DD)
    #[serde(rename = "expiryDate")]
    pub expiry_date: Option<String>,
    /// Manufacture date (YYYY-MM-DD)
    #[serde(rename = "manufactureDate")]
    pub manufacture_date: Option<String>,
    #[serde(rename = "locationId")]
    pub location_id: Option<String>,
    pub note: Option<String>,
    #[serde(rename = "taxPercentage", default, deserialize_with = "flex::opt_f64")]
    pub tax_percentage: Option<f64>,
    #[serde(rename = "totalBeforeTax", default, deserialize_with = "flex::opt_f64")]
    pub total_before_tax: Option<f64>,
    #[serde(rename = "itemVariantId")]
    pub item_variant_id: Option<String>,
    #[serde(rename = "vvmStatusId")]
    pub vvm_status_id: Option<String>,
    #[serde(rename = "donorId")]
    pub donor_id: Option<String>,
    #[serde(rename = "manufacturerId")]
    pub manufacturer_id: Option<String>,
    #[serde(rename = "campaignId")]
    pub campaign_id: Option<String>,
    #[serde(rename = "programId")]
    pub program_id: Option<String>,
    /// Link this received line back to a purchase order line
    #[serde(rename = "purchaseOrderLineId")]
    pub purchase_order_line_id: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateInboundShipmentLineParams {
    pub id: String,
    #[serde(rename = "itemId")]
    pub item_id: Option<String>,
    #[serde(rename = "packSize", default, deserialize_with = "flex::opt_f64")]
    pub pack_size: Option<f64>,
    #[serde(rename = "numberOfPacks", default, deserialize_with = "flex::opt_f64")]
    pub number_of_packs: Option<f64>,
    #[serde(rename = "costPricePerPack", default, deserialize_with = "flex::opt_f64")]
    pub cost_price_per_pack: Option<f64>,
    #[serde(rename = "sellPricePerPack", default, deserialize_with = "flex::opt_f64")]
    pub sell_price_per_pack: Option<f64>,
    pub batch: Option<String>,
    #[serde(rename = "expiryDate")]
    pub expiry_date: Option<String>,
    #[serde(rename = "manufactureDate")]
    pub manufacture_date: Option<String>,
    #[serde(rename = "locationId")]
    pub location_id: Option<String>,
    pub note: Option<String>,
    /// Line status (rare - normally set via shipment status)
    pub status: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Stocktake params --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertStocktakeParams {
    /// Create a stocktake covering all items in the store
    #[serde(rename = "isAllItemsStocktake", default, deserialize_with = "flex::opt_bool")]
    pub is_all_items_stocktake: Option<bool>,
    /// Limit stocktake to a master list (item catalog)
    #[serde(rename = "masterListId")]
    pub master_list_id: Option<String>,
    /// Include all master list items even those with no current stock
    #[serde(rename = "includeAllMasterListItems", default, deserialize_with = "flex::opt_bool")]
    pub include_all_master_list_items: Option<bool>,
    /// Limit stocktake to a specific storage location
    #[serde(rename = "locationId")]
    pub location_id: Option<String>,
    #[serde(rename = "vvmStatusId")]
    pub vvm_status_id: Option<String>,
    /// Only include stock lines expiring before this date (YYYY-MM-DD)
    #[serde(rename = "expiresBefore")]
    pub expires_before: Option<String>,
    /// First-ever stocktake for the store (creates baseline)
    #[serde(rename = "isInitialStocktake", default, deserialize_with = "flex::opt_bool")]
    pub is_initial_stocktake: Option<bool>,
    /// Create stocktake with no lines (add via insert_stocktake_line)
    #[serde(rename = "createBlankStocktake", default, deserialize_with = "flex::opt_bool")]
    pub create_blank_stocktake: Option<bool>,
    pub description: Option<String>,
    pub comment: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateStocktakeParams {
    pub id: String,
    /// Set to "FINALISED" to finalise the stocktake (only valid transition).
    pub status: Option<String>,
    /// Operational date for the stocktake (YYYY-MM-DD). This is the date the
    /// physical count took place -- the only date the GraphQL API lets you
    /// backdate. `createdDatetime` and `finalisedDatetime` are server-stamped.
    #[serde(rename = "stocktakeDate")]
    pub stocktake_date: Option<String>,
    pub description: Option<String>,
    pub comment: Option<String>,
    #[serde(rename = "isLocked", default, deserialize_with = "flex::opt_bool")]
    pub is_locked: Option<bool>,
    #[serde(rename = "countedBy")]
    pub counted_by: Option<String>,
    #[serde(rename = "verifiedBy")]
    pub verified_by: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertStocktakeLineParams {
    #[serde(rename = "stocktakeId")]
    pub stocktake_id: String,
    /// Existing stock line to count (if known); otherwise provide itemId + batch
    #[serde(rename = "stockLineId")]
    pub stock_line_id: Option<String>,
    #[serde(rename = "itemId")]
    pub item_id: Option<String>,
    /// Counted physical packs
    #[serde(rename = "countedNumberOfPacks", default, deserialize_with = "flex::opt_f64")]
    pub counted_number_of_packs: Option<f64>,
    pub batch: Option<String>,
    #[serde(rename = "expiryDate")]
    pub expiry_date: Option<String>,
    #[serde(rename = "manufactureDate")]
    pub manufacture_date: Option<String>,
    #[serde(rename = "packSize", default, deserialize_with = "flex::opt_f64")]
    pub pack_size: Option<f64>,
    #[serde(rename = "costPricePerPack", default, deserialize_with = "flex::opt_f64")]
    pub cost_price_per_pack: Option<f64>,
    #[serde(rename = "sellPricePerPack", default, deserialize_with = "flex::opt_f64")]
    pub sell_price_per_pack: Option<f64>,
    #[serde(rename = "locationId")]
    pub location_id: Option<String>,
    pub note: Option<String>,
    #[serde(rename = "reasonOptionId")]
    pub reason_option_id: Option<String>,
    #[serde(rename = "itemVariantId")]
    pub item_variant_id: Option<String>,
    #[serde(rename = "donorId")]
    pub donor_id: Option<String>,
    #[serde(rename = "manufacturerId")]
    pub manufacturer_id: Option<String>,
    #[serde(rename = "vvmStatusId")]
    pub vvm_status_id: Option<String>,
    #[serde(rename = "campaignId")]
    pub campaign_id: Option<String>,
    #[serde(rename = "programId")]
    pub program_id: Option<String>,
    pub comment: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateStocktakeLineParams {
    pub id: String,
    #[serde(rename = "countedNumberOfPacks", default, deserialize_with = "flex::opt_f64")]
    pub counted_number_of_packs: Option<f64>,
    #[serde(rename = "snapshotNumberOfPacks", default, deserialize_with = "flex::opt_f64")]
    pub snapshot_number_of_packs: Option<f64>,
    pub batch: Option<String>,
    #[serde(rename = "expiryDate")]
    pub expiry_date: Option<String>,
    #[serde(rename = "manufactureDate")]
    pub manufacture_date: Option<String>,
    #[serde(rename = "packSize", default, deserialize_with = "flex::opt_f64")]
    pub pack_size: Option<f64>,
    #[serde(rename = "costPricePerPack", default, deserialize_with = "flex::opt_f64")]
    pub cost_price_per_pack: Option<f64>,
    #[serde(rename = "sellPricePerPack", default, deserialize_with = "flex::opt_f64")]
    pub sell_price_per_pack: Option<f64>,
    #[serde(rename = "locationId")]
    pub location_id: Option<String>,
    pub note: Option<String>,
    #[serde(rename = "reasonOptionId")]
    pub reason_option_id: Option<String>,
    pub comment: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Purchase order params --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPurchaseOrdersParams {
    /// Status filter: NEW | REQUEST_APPROVAL | CONFIRMED | SENT | FINALISED
    pub status: Option<String>,
    #[serde(rename = "supplierId")]
    pub supplier_id: Option<String>,
    /// Sort by createdDatetime | number | status | supplier | sentDatetime (default createdDatetime)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    #[serde(default, deserialize_with = "flex::opt_bool")]
    pub desc: Option<bool>,
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub first: Option<u32>,
    #[serde(default, deserialize_with = "flex::opt_u32")]
    pub offset: Option<u32>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetPurchaseOrderParams {
    pub id: String,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertPurchaseOrderParams {
    /// Supplier ID (use search_names with isSupplier=true)
    #[serde(rename = "supplierId")]
    pub supplier_id: String,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdatePurchaseOrderParams {
    pub id: String,
    /// Status transition: NEW | REQUEST_APPROVAL | CONFIRMED | SENT | FINALISED
    pub status: Option<String>,
    #[serde(rename = "supplierId")]
    pub supplier_id: Option<String>,
    /// ISO datetime (YYYY-MM-DDTHH:MM:SS) — when the order was confirmed
    #[serde(rename = "confirmedDatetime")]
    pub confirmed_datetime: Option<String>,
    /// ISO datetime (YYYY-MM-DDTHH:MM:SS) — when the order was sent to supplier
    #[serde(rename = "sentDatetime")]
    pub sent_datetime: Option<String>,
    /// Date (YYYY-MM-DD) — when contract was signed
    #[serde(rename = "contractSignedDate")]
    pub contract_signed_date: Option<String>,
    /// Date (YYYY-MM-DD) — when advance payment was made
    #[serde(rename = "advancePaidDate")]
    pub advance_paid_date: Option<String>,
    /// Date (YYYY-MM-DD) — when goods arrived at port
    #[serde(rename = "receivedAtPortDate")]
    pub received_at_port_date: Option<String>,
    /// Date (YYYY-MM-DD) — when delivery is requested for
    #[serde(rename = "requestedDeliveryDate")]
    pub requested_delivery_date: Option<String>,
    pub comment: Option<String>,
    pub reference: Option<String>,
    #[serde(rename = "supplierDiscountPercentage", default, deserialize_with = "flex::opt_f64")]
    pub supplier_discount_percentage: Option<f64>,
    #[serde(rename = "supplierDiscountAmount", default, deserialize_with = "flex::opt_f64")]
    pub supplier_discount_amount: Option<f64>,
    #[serde(rename = "currencyId")]
    pub currency_id: Option<String>,
    #[serde(rename = "foreignExchangeRate", default, deserialize_with = "flex::opt_f64")]
    pub foreign_exchange_rate: Option<f64>,
    #[serde(rename = "shippingMethod")]
    pub shipping_method: Option<String>,
    #[serde(rename = "donorId")]
    pub donor_id: Option<String>,
    #[serde(rename = "supplierAgent")]
    pub supplier_agent: Option<String>,
    #[serde(rename = "authorisingOfficer1")]
    pub authorising_officer_1: Option<String>,
    #[serde(rename = "authorisingOfficer2")]
    pub authorising_officer_2: Option<String>,
    #[serde(rename = "additionalInstructions")]
    pub additional_instructions: Option<String>,
    #[serde(rename = "headingMessage")]
    pub heading_message: Option<String>,
    #[serde(rename = "agentCommission", default, deserialize_with = "flex::opt_f64")]
    pub agent_commission: Option<f64>,
    #[serde(rename = "documentCharge", default, deserialize_with = "flex::opt_f64")]
    pub document_charge: Option<f64>,
    #[serde(rename = "communicationsCharge", default, deserialize_with = "flex::opt_f64")]
    pub communications_charge: Option<f64>,
    #[serde(rename = "insuranceCharge", default, deserialize_with = "flex::opt_f64")]
    pub insurance_charge: Option<f64>,
    #[serde(rename = "freightCharge", default, deserialize_with = "flex::opt_f64")]
    pub freight_charge: Option<f64>,
    #[serde(rename = "freightConditions")]
    pub freight_conditions: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsertPurchaseOrderLineParams {
    #[serde(rename = "purchaseOrderId")]
    pub purchase_order_id: String,
    /// Either the item ID (UUID) or the item code (e.g. "AMOX250")
    #[serde(rename = "itemIdOrCode")]
    pub item_id_or_code: String,
    #[serde(rename = "requestedPackSize", default, deserialize_with = "flex::opt_f64")]
    pub requested_pack_size: Option<f64>,
    #[serde(rename = "requestedNumberOfUnits", default, deserialize_with = "flex::opt_f64")]
    pub requested_number_of_units: Option<f64>,
    /// Date YYYY-MM-DD
    #[serde(rename = "requestedDeliveryDate")]
    pub requested_delivery_date: Option<String>,
    /// Date YYYY-MM-DD
    #[serde(rename = "expectedDeliveryDate")]
    pub expected_delivery_date: Option<String>,
    #[serde(rename = "pricePerPackBeforeDiscount", default, deserialize_with = "flex::opt_f64")]
    pub price_per_pack_before_discount: Option<f64>,
    #[serde(rename = "pricePerPackAfterDiscount", default, deserialize_with = "flex::opt_f64")]
    pub price_per_pack_after_discount: Option<f64>,
    #[serde(rename = "manufacturerId")]
    pub manufacturer_id: Option<String>,
    pub note: Option<String>,
    pub unit: Option<String>,
    #[serde(rename = "supplierItemCode")]
    pub supplier_item_code: Option<String>,
    pub comment: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdatePurchaseOrderLineParams {
    pub id: String,
    #[serde(rename = "itemId")]
    pub item_id: Option<String>,
    #[serde(rename = "requestedPackSize", default, deserialize_with = "flex::opt_f64")]
    pub requested_pack_size: Option<f64>,
    #[serde(rename = "requestedNumberOfUnits", default, deserialize_with = "flex::opt_f64")]
    pub requested_number_of_units: Option<f64>,
    /// Requires AuthorisePurchaseOrder permission
    #[serde(rename = "adjustedNumberOfUnits", default, deserialize_with = "flex::opt_f64")]
    pub adjusted_number_of_units: Option<f64>,
    #[serde(rename = "requestedDeliveryDate")]
    pub requested_delivery_date: Option<String>,
    #[serde(rename = "expectedDeliveryDate")]
    pub expected_delivery_date: Option<String>,
    #[serde(rename = "pricePerPackBeforeDiscount", default, deserialize_with = "flex::opt_f64")]
    pub price_per_pack_before_discount: Option<f64>,
    #[serde(rename = "pricePerPackAfterDiscount", default, deserialize_with = "flex::opt_f64")]
    pub price_per_pack_after_discount: Option<f64>,
    #[serde(rename = "manufacturerId")]
    pub manufacturer_id: Option<String>,
    pub note: Option<String>,
    pub unit: Option<String>,
    #[serde(rename = "supplierItemCode")]
    pub supplier_item_code: Option<String>,
    pub comment: Option<String>,
    /// Line status: NEW | SENT | CLOSED
    pub status: Option<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeletePurchaseOrderLinesParams {
    /// Line IDs to delete (bulk)
    pub ids: Vec<String>,
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

// -------- Helpers --------

fn ok(text: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

fn err(e: AppError) -> Result<CallToolResult, McpError> {
    // Surface errors as text content with isError=true, matching the TS server's pattern.
    Ok(CallToolResult::error(vec![Content::text(format!(
        "Error: {e}"
    ))]))
}

// -------- Tool router --------

#[tool_router]
impl OmSupplyServer {
    #[tool(description = "List all available stores in the Open mSupply instance. Use this to discover store IDs needed by other tools.")]
    async fn list_stores(
        &self,
        Parameters(p): Parameters<ListStoresParams>,
    ) -> Result<CallToolResult, McpError> {
        match stores::list_stores(&self.client, p.first, p.offset).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get details of a specific store by its ID.")]
    async fn get_store(
        &self,
        Parameters(p): Parameters<GetStoreParams>,
    ) -> Result<CallToolResult, McpError> {
        match stores::get_store(&self.client, p.id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get the Open mSupply server version and connection status.")]
    async fn get_server_info(
        &self,
        _p: Parameters<EmptyParams>,
    ) -> Result<CallToolResult, McpError> {
        match stores::get_server_info(&self.client).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Search for items (products/medicines) by name or code. Returns item details including available stock on hand and consumption stats.")]
    async fn search_items(
        &self,
        Parameters(p): Parameters<SearchItemsParams>,
    ) -> Result<CallToolResult, McpError> {
        match items::search_items(
            &self.client,
            p.search,
            p.code,
            p.is_vaccine,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get detailed information about a specific item by its ID, including available batches and stock statistics.")]
    async fn get_item(
        &self,
        Parameters(p): Parameters<GetItemParams>,
    ) -> Result<CallToolResult, McpError> {
        match items::get_item(&self.client, p.item_id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get current stock levels showing available batches, quantities, expiry dates, and locations. Can filter by item, location, or availability.")]
    async fn get_stock_lines(
        &self,
        Parameters(p): Parameters<GetStockLinesParams>,
    ) -> Result<CallToolResult, McpError> {
        match stock::get_stock_lines(
            &self.client,
            p.item_id,
            p.search,
            p.location_id,
            p.has_stock,
            p.sort_by,
            p.desc,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get summary stock counts including expired items and items expiring soon. Useful for a quick overview of stock health.")]
    async fn get_stock_counts(
        &self,
        Parameters(p): Parameters<GetStockCountsParams>,
    ) -> Result<CallToolResult, McpError> {
        match stock::get_stock_counts(
            &self.client,
            p.days_till_expired,
            p.low_stock_threshold,
            p.high_stock_threshold,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get the transaction history (ledger) for a specific item, showing all stock movements in and out.")]
    async fn get_item_ledger(
        &self,
        Parameters(p): Parameters<GetItemLedgerParams>,
    ) -> Result<CallToolResult, McpError> {
        match stock::get_item_ledger(&self.client, p.item_id, p.first, p.offset, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "List invoices (shipments) with optional filters. Covers outbound shipments, inbound shipments, prescriptions, and returns.")]
    async fn list_invoices(
        &self,
        Parameters(p): Parameters<ListInvoicesParams>,
    ) -> Result<CallToolResult, McpError> {
        match invoices::list_invoices(
            &self.client,
            p.invoice_type,
            p.status,
            p.other_party_name,
            p.sort_by,
            p.desc,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get detailed information about a specific invoice including all line items, pricing, and party details.")]
    async fn get_invoice(
        &self,
        Parameters(p): Parameters<GetInvoiceParams>,
    ) -> Result<CallToolResult, McpError> {
        match invoices::get_invoice(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get counts of outbound shipments - how many created today/this week, and how many are not yet shipped.")]
    async fn get_outbound_shipment_counts(
        &self,
        Parameters(p): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match invoices::get_outbound_shipment_counts(&self.client, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get counts of inbound shipments - how many created today/this week, and how many are not yet delivered.")]
    async fn get_inbound_shipment_counts(
        &self,
        Parameters(p): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match invoices::get_inbound_shipment_counts(&self.client, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get a comprehensive dashboard summary of the store including stock health, shipment activity, and pending requisitions. Great for a quick overview.")]
    async fn get_dashboard_summary(
        &self,
        Parameters(p): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match dashboard::get_dashboard_summary(&self.client, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get counts of requisitions by status - draft requests, new responses, and emergency requisitions.")]
    async fn get_requisition_counts(
        &self,
        Parameters(p): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match dashboard::get_requisition_counts(&self.client, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Search for suppliers, customers, and facilities by name or code.")]
    async fn search_names(
        &self,
        Parameters(p): Parameters<SearchNamesParams>,
    ) -> Result<CallToolResult, McpError> {
        match dashboard::search_names(
            &self.client,
            p.search,
            p.code,
            p.is_supplier,
            p.is_customer,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get master lists (item catalogs) available for the store.")]
    async fn get_master_lists(
        &self,
        Parameters(p): Parameters<GetMasterListsParams>,
    ) -> Result<CallToolResult, McpError> {
        match dashboard::get_master_lists(&self.client, p.search, p.first, p.offset, p.store_id)
            .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Requisitions --------

    #[tool(description = "List requisitions (request and response) with optional filters by type, status, program, and other party.")]
    async fn list_requisitions(
        &self,
        Parameters(p): Parameters<ListRequisitionsParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::list_requisitions(
            &self.client,
            p.requisition_type,
            p.status,
            p.other_party_name,
            p.program_id,
            p.is_emergency,
            p.sort_by,
            p.desc,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get detailed information about a specific requisition including all lines, item stats, and other party.")]
    async fn get_requisition(
        &self,
        Parameters(p): Parameters<GetRequisitionParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::get_requisition(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Create a new draft request requisition (this store ordering stock from a supplier). Use search_names with isSupplier=true to find otherPartyId.")]
    async fn insert_request_requisition(
        &self,
        Parameters(p): Parameters<InsertRequestRequisitionParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::insert_request_requisition(
            &self.client,
            p.other_party_id,
            p.max_months_of_stock,
            p.min_months_of_stock,
            p.their_reference,
            p.comment,
            p.colour,
            p.expected_delivery_date,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update a request requisition. To submit/send a draft requisition to the supplier, set status to 'SENT'.")]
    async fn update_request_requisition(
        &self,
        Parameters(p): Parameters<UpdateRequestRequisitionParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::update_request_requisition(
            &self.client,
            p.id,
            p.status,
            p.comment,
            p.their_reference,
            p.colour,
            p.other_party_id,
            p.expected_delivery_date,
            p.max_months_of_stock,
            p.min_months_of_stock,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete a draft request requisition. Only allowed when status is DRAFT.")]
    async fn delete_request_requisition(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::delete_request_requisition(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Add an item line to a draft request requisition. Pass requestedQuantity to set the quantity in one call.")]
    async fn insert_request_requisition_line(
        &self,
        Parameters(p): Parameters<InsertRequestRequisitionLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::insert_request_requisition_line(
            &self.client,
            p.requisition_id,
            p.item_id,
            p.requested_quantity,
            p.comment,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update a line on a draft request requisition (requested quantity, comment, or reason).")]
    async fn update_request_requisition_line(
        &self,
        Parameters(p): Parameters<UpdateRequestRequisitionLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::update_request_requisition_line(
            &self.client,
            p.id,
            p.requested_quantity,
            p.comment,
            p.option_id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Remove a line from a draft request requisition.")]
    async fn delete_request_requisition_line(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match requisitions::delete_request_requisition_line(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- R&R forms --------

    #[tool(description = "List R&R (Report and Requisition) forms with optional program/period filters.")]
    async fn list_rnr_forms(
        &self,
        Parameters(p): Parameters<ListRnrFormsParams>,
    ) -> Result<CallToolResult, McpError> {
        match rnr::list_rnr_forms(
            &self.client,
            p.program_id,
            p.period_schedule_id,
            p.sort_by,
            p.desc,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get a specific R&R form including all line consumption and balance data.")]
    async fn get_rnr_form(
        &self,
        Parameters(p): Parameters<GetRnrFormParams>,
    ) -> Result<CallToolResult, McpError> {
        match rnr::get_rnr_form(&self.client, p.rnr_form_id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Create a new draft R&R form for a supplier, program, and reporting period. Use get_supplier_program_requisition_settings to discover valid combinations.")]
    async fn insert_rnr_form(
        &self,
        Parameters(p): Parameters<InsertRnrFormParams>,
    ) -> Result<CallToolResult, McpError> {
        match rnr::insert_rnr_form(
            &self.client,
            p.supplier_id,
            p.program_id,
            p.period_id,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update an R&R form. The 'lines' array REPLACES all existing lines -- fetch the current form, modify, send back.")]
    async fn update_rnr_form(
        &self,
        Parameters(p): Parameters<UpdateRnrFormParams>,
    ) -> Result<CallToolResult, McpError> {
        match rnr::update_rnr_form(
            &self.client,
            p.id,
            p.lines,
            p.their_reference,
            p.comment,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Finalise an R&R form -- submits it to the supplier. Status moves DRAFT -> FINALISED and the form becomes immutable.")]
    async fn finalise_rnr_form(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match rnr::finalise_rnr_form(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete a draft R&R form. Only allowed when status is DRAFT.")]
    async fn delete_rnr_form(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match rnr::delete_rnr_form(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Program / period discovery --------

    #[tool(description = "List programs available in this store (e.g. HIV, TB, immunisation programs). Needed to find programId for R&R forms.")]
    async fn list_programs(
        &self,
        Parameters(p): Parameters<ListProgramsParams>,
    ) -> Result<CallToolResult, McpError> {
        match programs::list_programs(
            &self.client,
            p.search,
            p.is_immunisation,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "List reporting periods, optionally filtered by program and date range. Needed to find periodId for R&R forms.")]
    async fn list_periods(
        &self,
        Parameters(p): Parameters<ListPeriodsParams>,
    ) -> Result<CallToolResult, McpError> {
        match programs::list_periods(
            &self.client,
            p.program_id,
            p.start_date_after,
            p.end_date_before,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get the program requisition settings configured for this store -- which programs are available, valid suppliers, order types, and available periods. The starting point for creating R&R forms or program requisitions.")]
    async fn get_supplier_program_requisition_settings(
        &self,
        Parameters(p): Parameters<StoreIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match programs::get_supplier_program_requisition_settings(&self.client, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Outbound shipments --------

    #[tool(description = "Create a new draft outbound shipment (this store issuing stock to a customer). Use search_names with isCustomer=true to find otherPartyId.")]
    async fn insert_outbound_shipment(
        &self,
        Parameters(p): Parameters<InsertOutboundShipmentParams>,
    ) -> Result<CallToolResult, McpError> {
        match outbound_shipments::insert_outbound_shipment(
            &self.client,
            p.other_party_id,
            p.on_hold,
            p.comment,
            p.their_reference,
            p.colour,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update an outbound shipment. Set status to advance through NEW->ALLOCATED->PICKED->SHIPPED. Pass `backdatedDatetime` (ISO 8601 UTC) alongside a status change to backdate the entire shipment chain — useful for seeding historic data.")]
    async fn update_outbound_shipment(
        &self,
        Parameters(p): Parameters<UpdateOutboundShipmentParams>,
    ) -> Result<CallToolResult, McpError> {
        match outbound_shipments::update_outbound_shipment(
            &self.client,
            p.id,
            p.status,
            p.on_hold,
            p.comment,
            p.their_reference,
            p.transport_reference,
            p.colour,
            p.expected_delivery_date,
            p.backdated_datetime,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete an outbound shipment. Only allowed while status is NEW.")]
    async fn delete_outbound_shipment(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match outbound_shipments::delete_outbound_shipment(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Add a stock line to an outbound shipment. Issues a specific batch from current stock — use get_stock_lines to find stockLineId.")]
    async fn insert_outbound_shipment_line(
        &self,
        Parameters(p): Parameters<InsertOutboundShipmentLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match outbound_shipments::insert_outbound_shipment_line(
            &self.client,
            p.invoice_id,
            p.stock_line_id,
            p.number_of_packs,
            p.tax_percentage,
            p.vvm_status_id,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update an outbound shipment line (quantity, batch, etc.).")]
    async fn update_outbound_shipment_line(
        &self,
        Parameters(p): Parameters<UpdateOutboundShipmentLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match outbound_shipments::update_outbound_shipment_line(
            &self.client,
            p.id,
            p.stock_line_id,
            p.number_of_packs,
            p.prescribed_quantity,
            p.vvm_status_id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Remove a line from an outbound shipment.")]
    async fn delete_outbound_shipment_line(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match outbound_shipments::delete_outbound_shipment_line(&self.client, p.id, p.store_id)
            .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Requisition fulfilment --------

    #[tool(description = "Commit to supply the full requested quantity on every line of a response requisition (sets supply_quantity = requested_quantity). REQUIRED once before create_requisition_shipment / fulfil_requisition on a freshly sent requisition — otherwise they report 'nothing to supply' because supply_quantity starts at 0. Mirrors the desktop 'Supply requested quantity' button. Run in the supplying store's context. (fulfil_requisition already does this step for you.)")]
    async fn supply_requested_quantity(
        &self,
        Parameters(p): Parameters<SupplyRequestedQuantityParams>,
    ) -> Result<CallToolResult, McpError> {
        match fulfil::supply_requested_quantity(
            &self.client,
            p.response_requisition_id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Fulfil a customer/response requisition by creating an outbound shipment LINKED to it (supplies the remaining-to-supply quantity of each line). Unlike insert_outbound_shipment, this link updates the requisition's supply status. Use list_requisitions/get_requisition (type RESPONSE) to find the id. Run in the supplying store's context. PREREQUISITE: call supply_requested_quantity first on a freshly sent requisition, or this reports 'nothing to supply'. Creates placeholder lines — follow with allocate_outbound_shipment_line (or use fulfil_requisition to do it all at once, which handles the supply step too).")]
    async fn create_requisition_shipment(
        &self,
        Parameters(p): Parameters<CreateRequisitionShipmentParams>,
    ) -> Result<CallToolResult, McpError> {
        match fulfil::create_requisition_shipment(
            &self.client,
            p.response_requisition_id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Allocate an unallocated outbound-shipment line to available stock using FEFO, automatically skipping expired, on-hold-location and unusable-VVM stock. Reports counts of what was allocated and what was skipped so you know if a line couldn't be fully covered. Needed before a shipment with placeholder lines can advance to SHIPPED.")]
    async fn allocate_outbound_shipment_line(
        &self,
        Parameters(p): Parameters<AllocateOutboundShipmentLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match fulfil::allocate_outbound_shipment_line(&self.client, p.line_id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "One-shot fulfil of a response requisition: supplies the requested quantity on every line, creates a linked outbound shipment, FEFO-allocates every created line to stock, and (if ship=true) advances it to SHIPPED. Returns the shipment summary plus a per-line allocation report, flagging any line left short because stock was skipped or insufficient. Convenience wrapper over supply_requested_quantity + create_requisition_shipment + allocate_outbound_shipment_line.")]
    async fn fulfil_requisition(
        &self,
        Parameters(p): Parameters<FulfilRequisitionParams>,
    ) -> Result<CallToolResult, McpError> {
        match fulfil::fulfil_requisition(
            &self.client,
            p.response_requisition_id,
            p.ship.unwrap_or(false),
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Inbound shipments (a.k.a. goods receipts) --------

    #[tool(description = "Create a new draft inbound shipment (this store receiving stock from a supplier). In Open mSupply, inbound shipments are how 'goods receipts' are represented. Use search_names with isSupplier=true.")]
    async fn insert_inbound_shipment(
        &self,
        Parameters(p): Parameters<InsertInboundShipmentParams>,
    ) -> Result<CallToolResult, McpError> {
        match inbound_shipments::insert_inbound_shipment(
            &self.client,
            p.other_party_id,
            p.on_hold,
            p.comment,
            p.their_reference,
            p.colour,
            p.requisition_id,
            p.purchase_order_id,
            p.insert_lines_from_purchase_order,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update an inbound shipment. Set status to advance through SHIPPED->DELIVERED->RECEIVED->VERIFIED. `receivedDatetime` is the ONLY backdate-able datetime — delivered/verified are server-stamped at the moment of transition.")]
    async fn update_inbound_shipment(
        &self,
        Parameters(p): Parameters<UpdateInboundShipmentParams>,
    ) -> Result<CallToolResult, McpError> {
        match inbound_shipments::update_inbound_shipment(
            &self.client,
            p.id,
            p.status,
            p.on_hold,
            p.comment,
            p.their_reference,
            p.colour,
            p.other_party_id,
            p.received_datetime,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete an inbound shipment. Only allowed while status is NEW.")]
    async fn delete_inbound_shipment(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match inbound_shipments::delete_inbound_shipment(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Add a received item line to an inbound shipment. Supports expiryDate and manufactureDate for realistic batch ageing — essential for seeding historic stock data.")]
    async fn insert_inbound_shipment_line(
        &self,
        Parameters(p): Parameters<InsertInboundShipmentLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match inbound_shipments::insert_inbound_shipment_line(
            &self.client,
            p.invoice_id,
            p.item_id,
            p.pack_size,
            p.number_of_packs,
            p.cost_price_per_pack,
            p.sell_price_per_pack,
            p.batch,
            p.expiry_date,
            p.manufacture_date,
            p.location_id,
            p.note,
            p.tax_percentage,
            p.total_before_tax,
            p.item_variant_id,
            p.vvm_status_id,
            p.donor_id,
            p.manufacturer_id,
            p.campaign_id,
            p.program_id,
            p.purchase_order_line_id,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update an inbound shipment line (batch, expiry, quantity, etc.).")]
    async fn update_inbound_shipment_line(
        &self,
        Parameters(p): Parameters<UpdateInboundShipmentLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match inbound_shipments::update_inbound_shipment_line(
            &self.client,
            p.id,
            p.item_id,
            p.pack_size,
            p.number_of_packs,
            p.cost_price_per_pack,
            p.sell_price_per_pack,
            p.batch,
            p.expiry_date,
            p.manufacture_date,
            p.location_id,
            p.note,
            p.status,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Remove a line from an inbound shipment.")]
    async fn delete_inbound_shipment_line(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match inbound_shipments::delete_inbound_shipment_line(&self.client, p.id, p.store_id)
            .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Stocktakes --------

    #[tool(description = "Create a new draft stocktake (physical inventory count). Can be limited by master list, location, or expiry filter. Use createBlankStocktake=true to start empty and add lines manually.")]
    async fn insert_stocktake(
        &self,
        Parameters(p): Parameters<InsertStocktakeParams>,
    ) -> Result<CallToolResult, McpError> {
        match stocktakes::insert_stocktake(
            &self.client,
            p.is_all_items_stocktake,
            p.master_list_id,
            p.include_all_master_list_items,
            p.location_id,
            p.vvm_status_id,
            p.expires_before,
            p.is_initial_stocktake,
            p.create_blank_stocktake,
            p.description,
            p.comment,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update a stocktake. Set `stocktakeDate` (YYYY-MM-DD) to record when the count was actually performed (the only date you can backdate). Set status='FINALISED' to commit counts to inventory.")]
    async fn update_stocktake(
        &self,
        Parameters(p): Parameters<UpdateStocktakeParams>,
    ) -> Result<CallToolResult, McpError> {
        match stocktakes::update_stocktake(
            &self.client,
            p.id,
            p.status,
            p.stocktake_date,
            p.description,
            p.comment,
            p.is_locked,
            p.counted_by,
            p.verified_by,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete a draft stocktake. Only allowed while status is NEW.")]
    async fn delete_stocktake(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match stocktakes::delete_stocktake(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Add a line to a stocktake. Either reference an existing stockLineId or provide itemId + batch + packSize. Set countedNumberOfPacks to record what was found.")]
    async fn insert_stocktake_line(
        &self,
        Parameters(p): Parameters<InsertStocktakeLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match stocktakes::insert_stocktake_line(
            &self.client,
            p.stocktake_id,
            p.stock_line_id,
            p.item_id,
            p.counted_number_of_packs,
            p.batch,
            p.expiry_date,
            p.manufacture_date,
            p.pack_size,
            p.cost_price_per_pack,
            p.sell_price_per_pack,
            p.location_id,
            p.note,
            p.reason_option_id,
            p.item_variant_id,
            p.donor_id,
            p.manufacturer_id,
            p.vvm_status_id,
            p.campaign_id,
            p.program_id,
            p.comment,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update a stocktake line (counted packs, batch, expiry, etc.).")]
    async fn update_stocktake_line(
        &self,
        Parameters(p): Parameters<UpdateStocktakeLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match stocktakes::update_stocktake_line(
            &self.client,
            p.id,
            p.counted_number_of_packs,
            p.snapshot_number_of_packs,
            p.batch,
            p.expiry_date,
            p.manufacture_date,
            p.pack_size,
            p.cost_price_per_pack,
            p.sell_price_per_pack,
            p.location_id,
            p.note,
            p.reason_option_id,
            p.comment,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Remove a line from a stocktake.")]
    async fn delete_stocktake_line(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match stocktakes::delete_stocktake_line(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    // -------- Purchase orders --------

    #[tool(description = "List purchase orders with optional filters by status and supplier.")]
    async fn list_purchase_orders(
        &self,
        Parameters(p): Parameters<ListPurchaseOrdersParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::list_purchase_orders(
            &self.client,
            p.status,
            p.supplier_id,
            p.sort_by,
            p.desc,
            p.first,
            p.offset,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Get a specific purchase order with all lines and date fields.")]
    async fn get_purchase_order(
        &self,
        Parameters(p): Parameters<GetPurchaseOrderParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::get_purchase_order(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Create a new draft purchase order for a supplier. Add lines with insert_purchase_order_line, then update status through NEW->CONFIRMED->SENT->FINALISED.")]
    async fn insert_purchase_order(
        &self,
        Parameters(p): Parameters<InsertPurchaseOrderParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::insert_purchase_order(
            &self.client,
            p.supplier_id,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update a purchase order: status, dates, charges, references, etc. Backdate-able fields: confirmedDatetime, sentDatetime (ISO datetime), contractSignedDate, advancePaidDate, receivedAtPortDate, requestedDeliveryDate (YYYY-MM-DD). Note: createdDatetime and finalisedDatetime are NOT settable.")]
    async fn update_purchase_order(
        &self,
        Parameters(p): Parameters<UpdatePurchaseOrderParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::update_purchase_order(
            &self.client,
            p.id,
            p.status,
            p.supplier_id,
            p.confirmed_datetime,
            p.sent_datetime,
            p.contract_signed_date,
            p.advance_paid_date,
            p.received_at_port_date,
            p.requested_delivery_date,
            p.comment,
            p.reference,
            p.supplier_discount_percentage,
            p.supplier_discount_amount,
            p.currency_id,
            p.foreign_exchange_rate,
            p.shipping_method,
            p.donor_id,
            p.supplier_agent,
            p.authorising_officer_1,
            p.authorising_officer_2,
            p.additional_instructions,
            p.heading_message,
            p.agent_commission,
            p.document_charge,
            p.communications_charge,
            p.insurance_charge,
            p.freight_charge,
            p.freight_conditions,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete a draft purchase order.")]
    async fn delete_purchase_order(
        &self,
        Parameters(p): Parameters<DeleteByIdParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::delete_purchase_order(&self.client, p.id, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Add a line to a purchase order. Pass itemIdOrCode (item UUID or code), quantities, prices, and optional dates.")]
    async fn insert_purchase_order_line(
        &self,
        Parameters(p): Parameters<InsertPurchaseOrderLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::insert_purchase_order_line(
            &self.client,
            p.purchase_order_id,
            p.item_id_or_code,
            p.requested_pack_size,
            p.requested_number_of_units,
            p.requested_delivery_date,
            p.expected_delivery_date,
            p.price_per_pack_before_discount,
            p.price_per_pack_after_discount,
            p.manufacturer_id,
            p.note,
            p.unit,
            p.supplier_item_code,
            p.comment,
            p.id,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Update a purchase order line (quantity, price, status NEW/SENT/CLOSED, dates).")]
    async fn update_purchase_order_line(
        &self,
        Parameters(p): Parameters<UpdatePurchaseOrderLineParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::update_purchase_order_line(
            &self.client,
            p.id,
            p.item_id,
            p.requested_pack_size,
            p.requested_number_of_units,
            p.adjusted_number_of_units,
            p.requested_delivery_date,
            p.expected_delivery_date,
            p.price_per_pack_before_discount,
            p.price_per_pack_after_discount,
            p.manufacturer_id,
            p.note,
            p.unit,
            p.supplier_item_code,
            p.comment,
            p.status,
            p.store_id,
        )
        .await
        {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Delete one or more purchase order lines (bulk operation, takes an array of IDs).")]
    async fn delete_purchase_order_lines(
        &self,
        Parameters(p): Parameters<DeletePurchaseOrderLinesParams>,
    ) -> Result<CallToolResult, McpError> {
        match purchase_orders::delete_purchase_order_lines(&self.client, p.ids, p.store_id).await {
            Ok(t) => ok(t),
            Err(e) => err(e),
        }
    }
}

#[tool_handler]
impl ServerHandler for OmSupplyServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2024_11_05;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "Query Open mSupply inventory, stock, shipments, invoices, and dashboard data.".into(),
        );
        info
    }
}

#[cfg(test)]
mod flex_tests {
    use super::*;

    #[test]
    fn stringified_scalars_deserialize() {
        // Simulates a bridge that sends every arg as a JSON string.
        let json = serde_json::json!({
            "search": "amox",
            "isVaccine": "true",
            "first": "25",
            "offset": "0"
        });
        let p: SearchItemsParams = serde_json::from_value(json).unwrap();
        assert_eq!(p.is_vaccine, Some(true));
        assert_eq!(p.first, Some(25));
        assert_eq!(p.offset, Some(0));
    }

    #[test]
    fn native_scalars_still_deserialize() {
        let json = serde_json::json!({ "isVaccine": false, "first": 10 });
        let p: SearchItemsParams = serde_json::from_value(json).unwrap();
        assert_eq!(p.is_vaccine, Some(false));
        assert_eq!(p.first, Some(10));
    }

    #[test]
    fn missing_optionals_are_none() {
        let p: SearchItemsParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p.is_vaccine, None);
        assert_eq!(p.first, None);
    }

    #[test]
    fn required_f64_accepts_string() {
        let json = serde_json::json!({
            "otherPartyId": "abc",
            "maxMonthsOfStock": "6.0",
            "minMonthsOfStock": "3"
        });
        let p: InsertRequestRequisitionParams = serde_json::from_value(json).unwrap();
        assert_eq!(p.max_months_of_stock, 6.0);
        assert_eq!(p.min_months_of_stock, 3.0);
    }
}
