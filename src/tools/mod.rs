//! MCP tool router -- maps the 16 tools exposed by the TypeScript server onto rmcp.
//!
//! Each `#[tool]` method is a thin wrapper that:
//! 1. destructures typed parameters (via `Parameters<T>`),
//! 2. delegates to a free function in the relevant sub-module,
//! 3. converts `Result<String, AppError>` into the MCP `CallToolResult` envelope.

mod dashboard;
mod invoices;
mod items;
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
