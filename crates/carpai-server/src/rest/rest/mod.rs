pub mod server;
#[path = "../api/rest_api.rs"]
pub mod rest_api;

pub use server::{RestServer, CompleteRequest, CompleteResponse, GenerateRequest, GenerateResponse};
pub use rest_api::{ApiState, create_router as create_rest_router};