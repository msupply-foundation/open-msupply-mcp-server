//! MCP tool router -- maps the 16 tools exposed by the TypeScript server onto rmcp.
//!
//! Each `#[tool]` method is a thin wrapper that:
//! 1. destructures typed parameters (via `Parameters<T>`),
//! 2. delegates to a free function in the relevant sub-module,
//! 3. converts `Result<String, AppError>` into the MCP `CallToolResult` envelope.

mod dashboard;
mod invoices;
mod items;
mod programs;
mod requisitions;
mod rnr;
mod stock;
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
    pub first: Option<u32>,
    /// Number of results to skip for pagination
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
    #[serde(rename = "isVaccine")]
    pub is_vaccine: Option<bool>,
    /// Max results to return (default 25)
    pub first: Option<u32>,
    /// Number of results to skip for pagination
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
    #[serde(rename = "hasStock")]
    pub has_stock: Option<bool>,
    /// Sort field: expiryDate | itemName | itemCode | batch | numberOfPacks (default: itemName)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    /// Sort descending
    pub desc: Option<bool>,
    /// Max results (default 25)
    pub first: Option<u32>,
    /// Pagination offset
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
    #[serde(rename = "storeId")]
    pub store_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetStockCountsParams {
    /// Days-until-expired threshold (default 30)
    #[serde(rename = "daysTillExpired")]
    pub days_till_expired: Option<i32>,
    /// Months-of-stock below which items are "low stock" (default 3)
    #[serde(rename = "lowStockThreshold")]
    pub low_stock_threshold: Option<f64>,
    /// Months-of-stock above which items are "high stock" (default 6)
    #[serde(rename = "highStockThreshold")]
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
    pub first: Option<u32>,
    /// Pagination offset
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
    pub desc: Option<bool>,
    /// Max results (default 25)
    pub first: Option<u32>,
    /// Pagination offset
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
    #[serde(rename = "isSupplier")]
    pub is_supplier: Option<bool>,
    /// Filter to only customers
    #[serde(rename = "isCustomer")]
    pub is_customer: Option<bool>,
    /// Max results (default 25)
    pub first: Option<u32>,
    /// Pagination offset
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
    pub first: Option<u32>,
    /// Pagination offset
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
    #[serde(rename = "isEmergency")]
    pub is_emergency: Option<bool>,
    /// Sort field: requisitionNumber | type | status | otherPartyName | sentDatetime | createdDatetime | finalisedDatetime | expectedDeliveryDate | theirReference | orderType | programName | periodStartDate | comment (default: createdDatetime)
    #[serde(rename = "sortBy")]
    pub sort_by: Option<String>,
    /// Sort descending (default true)
    pub desc: Option<bool>,
    /// Max results (default 25)
    pub first: Option<u32>,
    /// Pagination offset
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
    #[serde(rename = "maxMonthsOfStock")]
    pub max_months_of_stock: f64,
    /// Minimum months of stock threshold (e.g. 3.0)
    #[serde(rename = "minMonthsOfStock")]
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
    #[serde(rename = "maxMonthsOfStock")]
    pub max_months_of_stock: Option<f64>,
    /// Minimum months of stock
    #[serde(rename = "minMonthsOfStock")]
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
    #[serde(rename = "requestedQuantity")]
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
    #[serde(rename = "requestedQuantity")]
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
    pub desc: Option<bool>,
    /// Max results (default 25)
    pub first: Option<u32>,
    /// Pagination offset
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
    #[serde(rename = "isImmunisation")]
    pub is_immunisation: Option<bool>,
    /// Max results (default 25)
    pub first: Option<u32>,
    /// Pagination offset
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
    pub first: Option<u32>,
    /// Pagination offset
    pub offset: Option<u32>,
    /// Store ID (uses default if not provided)
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
