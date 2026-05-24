//! Health check service (gRPC health protocol)

use crate::grpc::carpai::health::{
    health_server::{Health, HealthServer},
    HealthCheckRequest, HealthCheckResponse,
};
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Debug, Default)]
pub struct HealthServiceImpl;

#[tonic::async_trait]
impl Health for HealthServiceImpl {
    async fn check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        info!("Received health check request for service: {}", request.get_ref().service);
        Ok(Response::new(HealthCheckResponse {
            status: HealthCheckResponse::ServingStatus::Serving as i32,
        }))
    }

    type WatchStream = std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<HealthCheckResponse, Status>> + Send + 'static>>;

    async fn watch(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        info!("Received health watch request");
        // TODO: Implement streaming health checks
        Err(Status::unimplemented("watch not yet implemented"))
    }
}

pub fn create_health_service() -> HealthServer<HealthServiceImpl> {
    HealthServer::new(HealthServiceImpl)
}
