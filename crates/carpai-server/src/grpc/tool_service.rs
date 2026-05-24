//! Tool execution RPC handler

use crate::grpc::carpai::tool::{
    tool_service_server::{ToolService, ToolServiceServer},
    ExecuteToolRequest, ExecuteToolResponse, ListToolsRequest,
    ListToolsResponse, GetToolDefinitionRequest, ToolDefinition,
};
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Debug, Default)]
pub struct ToolServiceImpl;

#[tonic::async_trait]
impl ToolService for ToolServiceImpl {
    async fn execute_tool(
        &self,
        request: Request<ExecuteToolRequest>,
    ) -> Result<Response<ExecuteToolResponse>, Status> {
        info!("Received execute tool request");
        Err(Status::unimplemented("execute_tool not yet implemented"))
    }

    async fn list_tools(
        &self,
        request: Request<ListToolsRequest>,
    ) -> Result<Response<ListToolsResponse>, Status> {
        info!("Received list tools request");
        Err(Status::unimplemented("list_tools not yet implemented"))
    }

    async fn get_tool_definition(
        &self,
        request: Request<GetToolDefinitionRequest>,
    ) -> Result<Response<ToolDefinition>, Status> {
        info!("Received get tool definition request");
        Err(Status::unimplemented("get_tool_definition not yet implemented"))
    }
}

pub fn create_tool_service() -> ToolServiceServer<ToolServiceImpl> {
    ToolServiceServer::new(ToolServiceImpl)
}
