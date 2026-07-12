use std::sync::Arc;

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ErrorData, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::{RequestContext, RoleServer},
    ServerHandler,
};
use serde_json::Value;
use tauri::AppHandle;

use crate::agents::{AgentContext, AgentRuntime};

use super::record_call;

pub(crate) const READ_ONLY_TOOLS: &[&str] = &["read_note", "search_notes", "get_link_suggestions"];

#[derive(Clone)]
pub(crate) struct SmoothMcpHandler {
    app: AppHandle,
    runtime: AgentRuntime,
}

impl SmoothMcpHandler {
    pub(crate) fn new(app: AppHandle, runtime: AgentRuntime) -> Self {
        Self { app, runtime }
    }

    fn tools(&self) -> Vec<Tool> {
        self.runtime
            .list_tools()
            .into_iter()
            .filter(|tool| READ_ONLY_TOOLS.contains(&tool.name.as_str()))
            .filter_map(|tool| {
                let schema = tool.input_schema.as_object()?.clone();
                Some(Tool::new(tool.name, tool.description, Arc::new(schema)))
            })
            .collect()
    }
}

impl ServerHandler for SmoothMcpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("smooth", env!("CARGO_PKG_VERSION"))
                    .with_title("Smooth Notes"),
            )
            .with_instructions(
                "Read-only access to the user's local Smooth notes. Use exact note IDs returned by search_notes."
                    .to_string(),
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = ListToolsResult::default();
        result.tools = self.tools();
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if !READ_ONLY_TOOLS.contains(&request.name.as_ref()) {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Tool '{}' is not available through the read-only MCP server",
                request.name
            ))]));
        }

        let input = Value::Object(request.arguments.unwrap_or_default());
        let ctx = AgentContext::new(self.app.clone());
        match self
            .runtime
            .execute_tool(&request.name, input.clone(), &ctx)
            .await
        {
            Ok(output) => {
                record_call(&self.app, &request.name, &input, None);
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    output.to_string(),
                )]))
            }
            Err(error) => {
                let message = error.to_string();
                record_call(&self.app, &request.name, &input, Some(&message));
                Ok(CallToolResult::error(vec![ContentBlock::text(message)]))
            }
        }
    }
}
