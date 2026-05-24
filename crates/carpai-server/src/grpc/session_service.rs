//! Session CRUD RPC handler

use crate::grpc::carpai::session::{
    session_service_server::{SessionService, SessionServiceServer},
    CreateSessionRequest, SessionResponse, GetSessionRequest,
    ListSessionsRequest, ListSessionsResponse, DeleteSessionRequest,
    DeleteSessionResponse, UpdateSessionRequest,
};
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Debug, Default)]
pub struct SessionServiceImpl;

#[tonic::async_trait]
impl SessionService for SessionServiceImpl {
    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<SessionResponse>, Status> {
        info!("Received create session request");
        Err(Status::unimplemented("create_session not yet implemented"))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<SessionResponse>, Status> {
        info!("Received get session request");
        Err(Status::unimplemented("get_session not yet implemented"))
    }

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        info!("Received list sessions request");
        Err(Status::unimplemented("list_sessions not yet implemented"))
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        info!("Received delete session request");
        Err(Status::unimplemented("delete_session not yet implemented"))
    }

    async fn update_session(
        &self,
        request: Request<UpdateSessionRequest>,
    ) -> Result<Response<SessionResponse>, Status> {
        info!("Received update session request");
        Err(Status::unimplemented("update_session not yet implemented"))
    }
}

pub fn create_session_service() -> SessionServiceServer<SessionServiceImpl> {
    SessionServiceServer::new(SessionServiceImpl)
}
