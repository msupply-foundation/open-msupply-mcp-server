//! Fulfilment tools — link an outbound shipment to a response requisition and
//! allocate its placeholder lines to real stock.
//!
//! A standalone `insert_outbound_shipment` is NOT linked to the requisition that
//! requested the goods, so the supplying store's response requisition stays at
//! `status: NEW, supplyQuantity: 0`. These tools surface the server's
//! `createRequisitionShipment` (which creates a *linked* shipment) and
//! `allocateOutboundShipmentUnallocatedLine` (FEFO stock picking) so the full
//! "fulfil this order" path can run through the MCP.
//!
//! Full fulfil path:
//!   supply_requested_quantity -> create_requisition_shipment ->
//!   allocate each created line -> ship.
//! `fulfil_requisition` chains all of these in one call. The supply step is the
//! easily-missed prerequisite: a freshly sent response requisition has
//! supply_quantity = 0 on every line, so create_requisition_shipment would
//! otherwise report "nothing to supply".

use crate::client::OmSupplyClient;
use crate::error::AppError;
use serde_json::{Value, json};

const SUPPLY_REQUESTED_QUANTITY_MUTATION: &str = r#"
  mutation supplyRequestedQuantity(
    $storeId: String
    $input: SupplyRequestedQuantityInput!
  ) {
    supplyRequestedQuantity(storeId: $storeId, input: $input) {
      __typename
      ... on RequisitionLineConnector { totalCount }
      ... on SupplyRequestedQuantityError {
        error { __typename description }
      }
    }
  }
"#;

const CREATE_REQUISITION_SHIPMENT_MUTATION: &str = r#"
  mutation createRequisitionShipment(
    $storeId: String
    $input: CreateRequisitionShipmentInput!
  ) {
    createRequisitionShipment(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceNode {
        id invoiceNumber status type otherPartyName
        requisition { id }
        lines {
          totalCount
          nodes { id itemName numberOfPacks packSize type }
        }
      }
      ... on CreateRequisitionShipmentError {
        error { __typename description }
      }
    }
  }
"#;

const ALLOCATE_UNALLOCATED_LINE_MUTATION: &str = r#"
  mutation allocateOutboundShipmentUnallocatedLine($storeId: String!, $lineId: String!) {
    allocateOutboundShipmentUnallocatedLine(storeId: $storeId, lineId: $lineId) {
      __typename
      ... on AllocateOutboundShipmentUnallocatedLineNode {
        inserts { totalCount }
        updates { totalCount }
        deletes { id }
        skippedExpiredStockLines { totalCount }
        skippedOnHoldStockLines { totalCount }
        skippedUnusableVvmStatusLines { totalCount }
        issuedExpiringSoonStockLines { totalCount }
      }
      ... on AllocateOutboundShipmentUnallocatedLineError {
        error { __typename description }
      }
    }
  }
"#;

const SHIP_OUTBOUND_SHIPMENT_MUTATION: &str = r#"
  mutation updateOutboundShipment($storeId: String!, $input: UpdateOutboundShipmentInput!) {
    updateOutboundShipment(storeId: $storeId, input: $input) {
      __typename
      ... on InvoiceNode { id invoiceNumber status shippedDatetime }
      ... on UpdateOutboundShipmentError { error { __typename description } }
      ... on NodeError { error { __typename description } }
    }
  }
"#;

/// Turn a `createRequisitionShipment` error node into a friendly message.
fn map_create_error(response: &Value) -> AppError {
    let err_typename = response
        .pointer("/error/__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let desc = response
        .pointer("/error/description")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");

    let msg = match err_typename {
        "RecordNotFound" => "Requisition not found".to_string(),
        "CannotEditRequisition" => {
            "Requisition is not editable (already finalised, or supplier store disabled)".to_string()
        }
        "NothingRemainingToSupply" => {
            "Nothing left to supply — this requisition is already fulfilled (check its linked shipments)"
                .to_string()
        }
        // NotThisStoreRequisition / NotAResponseRequisition / anything else: surface the description.
        _ => desc.to_string(),
    };
    AppError::Graphql(msg)
}

/// Commit supply_quantity = requested_quantity on every line of a response
/// requisition. Returns the number of updated lines.
async fn do_supply_requested_quantity(
    client: &OmSupplyClient,
    response_requisition_id: String,
    store_id: String,
) -> Result<i64, AppError> {
    let data: Value = client
        .query(
            SUPPLY_REQUESTED_QUANTITY_MUTATION,
            json!({
                "storeId": store_id,
                "input": { "responseRequisitionId": response_requisition_id },
            }),
        )
        .await?;

    let response = data
        .get("supplyRequestedQuantity")
        .ok_or_else(|| AppError::UnexpectedResponse("missing supplyRequestedQuantity".into()))?;

    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if typename == "RequisitionLineConnector" {
        return Ok(response
            .get("totalCount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0));
    }

    let err_typename = response
        .pointer("/error/__typename")
        .and_then(|v| v.as_str())
        .unwrap_or(typename);
    let desc = response
        .pointer("/error/description")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    let msg = match err_typename {
        "RecordNotFound" => "Requisition not found".to_string(),
        "CannotEditRequisition" => {
            "Requisition is not editable (already finalised, or supplier store disabled)".to_string()
        }
        // NotThisStoreRequisition / anything else: surface the description.
        _ => desc.to_string(),
    };
    Err(AppError::Graphql(msg))
}

/// Run the linked-shipment mutation, returning the created `InvoiceNode`.
async fn do_create_requisition_shipment(
    client: &OmSupplyClient,
    response_requisition_id: String,
    store_id: String,
) -> Result<Value, AppError> {
    let data: Value = client
        .query(
            CREATE_REQUISITION_SHIPMENT_MUTATION,
            json!({
                "storeId": store_id,
                "input": { "responseRequisitionId": response_requisition_id },
            }),
        )
        .await?;

    let response = data
        .get("createRequisitionShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing createRequisitionShipment".into()))?;

    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if typename == "InvoiceNode" {
        Ok(response.clone())
    } else {
        Err(map_create_error(response))
    }
}

/// Run the FEFO allocate mutation, returning the allocation result node.
async fn do_allocate_line(
    client: &OmSupplyClient,
    line_id: String,
    store_id: String,
) -> Result<Value, AppError> {
    let data: Value = client
        .query(
            ALLOCATE_UNALLOCATED_LINE_MUTATION,
            json!({ "storeId": store_id, "lineId": line_id }),
        )
        .await?;

    let response = data
        .get("allocateOutboundShipmentUnallocatedLine")
        .ok_or_else(|| {
            AppError::UnexpectedResponse("missing allocateOutboundShipmentUnallocatedLine".into())
        })?;

    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if typename == "AllocateOutboundShipmentUnallocatedLineNode" {
        Ok(response.clone())
    } else {
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
}

fn total_count(node: &Value, field: &str) -> i64 {
    node.pointer(&format!("/{field}/totalCount"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

fn deletes_count(node: &Value) -> usize {
    node.get("deletes")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Human-readable summary of one allocation result.
fn format_allocation(node: &Value) -> String {
    format!(
        "  allocatedStockLines (inserts): {inserts}\n  \
           updatedLines: {updates}\n  \
           removedPlaceholders (deletes): {deletes}\n  \
           skippedExpired: {exp}\n  \
           skippedOnHold: {hold}\n  \
           skippedUnusableVvm: {vvm}\n  \
           issuedExpiringSoon: {soon}",
        inserts = total_count(node, "inserts"),
        updates = total_count(node, "updates"),
        deletes = deletes_count(node),
        exp = total_count(node, "skippedExpiredStockLines"),
        hold = total_count(node, "skippedOnHoldStockLines"),
        vvm = total_count(node, "skippedUnusableVvmStatusLines"),
        soon = total_count(node, "issuedExpiringSoonStockLines"),
    )
}

/// True if the allocation actually issued any stock (inserts > 0).
fn allocation_issued_stock(node: &Value) -> bool {
    total_count(node, "inserts") > 0
}

/// Format the created linked shipment as the spec's success summary.
fn format_shipment_summary(node: &Value) -> String {
    let get = |k: &str| node.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let requisition_id = node
        .pointer("/requisition/id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let line_count = node
        .pointer("/lines/totalCount")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    format!(
        "  id: {id}\n  \
           invoiceNumber: {number}\n  \
           otherPartyName: {party}\n  \
           status: {status}\n  \
           type: {typ}\n  \
           requisitionId: {req}\n  \
           lineCount: {count}",
        id = get("id"),
        number = node
            .get("invoiceNumber")
            .map(|v| v.to_string())
            .unwrap_or_default(),
        party = get("otherPartyName"),
        status = get("status"),
        typ = get("type"),
        req = requisition_id,
        count = line_count,
    )
}

// -------- Tool D (prerequisite) --------

pub async fn supply_requested_quantity(
    client: &OmSupplyClient,
    response_requisition_id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let count =
        do_supply_requested_quantity(client, response_requisition_id, resolved_store_id).await?;
    Ok(format!(
        "Committed to supply the requested quantity on {count} line(s). \
         The requisition is now ready for create_requisition_shipment / fulfil_requisition."
    ))
}

// -------- Tool A --------

pub async fn create_requisition_shipment(
    client: &OmSupplyClient,
    response_requisition_id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let node =
        do_create_requisition_shipment(client, response_requisition_id, resolved_store_id).await?;
    Ok(format!(
        "Requisition shipment created:\n{}",
        format_shipment_summary(&node)
    ))
}

// -------- Tool B --------

pub async fn allocate_outbound_shipment_line(
    client: &OmSupplyClient,
    line_id: String,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;
    let node = do_allocate_line(client, line_id.clone(), resolved_store_id).await?;
    Ok(format!(
        "Allocated line (id={line_id}):\n{}",
        format_allocation(&node)
    ))
}

// -------- Tool C --------

pub async fn fulfil_requisition(
    client: &OmSupplyClient,
    response_requisition_id: String,
    ship: bool,
    store_id: Option<String>,
) -> Result<String, AppError> {
    let resolved_store_id = client.require_store_id(store_id).await?;

    // 1. Commit supply_quantity = requested_quantity on every line. On a freshly
    // sent requisition supply_quantity is 0, so without this the create step
    // reports "nothing to supply". Mirrors the desktop "Supply requested
    // quantity" button.
    let supplied = do_supply_requested_quantity(
        client,
        response_requisition_id.clone(),
        resolved_store_id.clone(),
    )
    .await?;

    // 2. Create the linked shipment.
    let shipment =
        do_create_requisition_shipment(client, response_requisition_id, resolved_store_id.clone())
            .await?;
    let shipment_id = shipment
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut out = vec![
        format!("Supplied requested quantity on {supplied} line(s)."),
        String::new(),
        "Requisition shipment created:".to_string(),
        format_shipment_summary(&shipment),
        String::new(),
        "Line allocation:".to_string(),
    ];

    // 2. Allocate each created (placeholder) line.
    let lines = shipment
        .pointer("/lines/nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut any_short = false;
    if lines.is_empty() {
        out.push("  (no lines were created)".to_string());
    }
    for line in &lines {
        let line_id = line.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let item_name = line.get("itemName").and_then(|v| v.as_str()).unwrap_or("?");
        match do_allocate_line(client, line_id.to_string(), resolved_store_id.clone()).await {
            Ok(alloc) => {
                if !allocation_issued_stock(&alloc) {
                    any_short = true;
                }
                out.push(format!("  {item_name} (line {line_id}):"));
                // Indent the allocation block one extra level.
                for l in format_allocation(&alloc).lines() {
                    out.push(format!("  {l}"));
                }
            }
            Err(e) => {
                any_short = true;
                out.push(format!("  {item_name} (line {line_id}): allocation failed — {e}"));
            }
        }
    }

    // 3. Optionally ship.
    if ship {
        out.push(String::new());
        if any_short {
            out.push(
                "Not shipped: one or more lines could not be fully allocated (all stock \
                 skipped or insufficient). Resolve stock, allocate the remaining lines, then \
                 update the shipment to SHIPPED."
                    .to_string(),
            );
        } else {
            match do_ship_shipment(client, shipment_id.clone(), resolved_store_id.clone()).await {
                Ok(status) => out.push(format!("Shipment marked SHIPPED (status: {status}).")),
                Err(e) => out.push(format!("Ship step failed — {e}")),
            }
        }
    }

    Ok(out.join("\n"))
}

/// Advance the shipment to SHIPPED.
async fn do_ship_shipment(
    client: &OmSupplyClient,
    shipment_id: String,
    store_id: String,
) -> Result<String, AppError> {
    let data: Value = client
        .query(
            SHIP_OUTBOUND_SHIPMENT_MUTATION,
            json!({
                "storeId": store_id,
                "input": { "id": shipment_id, "status": "SHIPPED" },
            }),
        )
        .await?;

    let response = data
        .get("updateOutboundShipment")
        .ok_or_else(|| AppError::UnexpectedResponse("missing updateOutboundShipment".into()))?;

    let typename = response
        .get("__typename")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if typename == "InvoiceNode" {
        Ok(response
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("SHIPPED")
            .to_string())
    } else {
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
}
