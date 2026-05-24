//! gRPC server implementation

// Include generated protobuf code
pub mod carpai {
    pub mod agent {
        tonic::include_proto!("carpai.agent");
    }
    pub mod session {
        tonic::include_proto!("carpai.session");
    }
    pub mod tool {
        tonic::include_proto!("carpai.tool");
    }
    pub mod health {
        tonic::include_proto!("carpai.health");
    }
}

pub mod server;
pub mod agent_service;
pub mod session_service;
pub mod tool_service;
pub mod health_service;

pub use server::grpc_server;
