use std::net::SocketAddr;
use tonic::transport::Server;
use tonic::Request;

use jcode::grpc::{GrpcServerBuilder, proto};

struct TestClient {
    client: proto::open_code_service_client::OpenCodeServiceClient<tonic::transport::Channel>,
}

impl TestClient {
    async fn new(addr: &SocketAddr) -> Self {
        let client = proto::open_code_service_client::OpenCodeServiceClient::connect(format!("http://{}", addr))
            .await
            .expect("Failed to connect to server");
        Self { client }
    }
}

#[tokio::test]
async fn test_parse_ast() {
    let code = r#"pub struct User {
    name: String,
    age: u32,
}

pub fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}"#;

    let request = Request::new(proto::ParseAstRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        language: "rust".to_string(),
    });

    let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.parse_ast(request).await.unwrap();
    
    assert!(response.into_inner().node_count > 0);
}

#[tokio::test]
async fn test_infer_types() {
    let code = r#"let x: i32 = 42;
let y = "hello";

pub struct Point {
    x: f64,
    y: f64,
}"#;

    let request = Request::new(proto::InferTypesRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
    });

    let addr: SocketAddr = "127.0.0.1:50052".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.infer_types(request).await.unwrap();
    
    assert!(!response.into_inner().types.is_empty());
}

#[tokio::test]
async fn test_resolve_symbols() {
    let code = r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

let result = add(1, 2);"#;

    let request = Request::new(proto::ResolveSymbolsRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        include_definitions: true,
        include_references: true,
    });

    let addr: SocketAddr = "127.0.0.1:50053".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.resolve_symbols(request).await.unwrap();
    
    assert!(response.into_inner().resolved_count > 0);
}

#[tokio::test]
async fn test_validate_code() {
    let code = r#"fn test() {
    let x = 5
    println!("Hello")
}"#;

    let request = Request::new(proto::ValidateCodeRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        language: "rust".to_string(),
        check_syntax: true,
        check_types: true,
        check_style: true,
    });

    let addr: SocketAddr = "127.0.0.1:50054".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.validate_code(request).await.unwrap();
    
    assert!(response.into_inner().warning_count > 0);
}

#[tokio::test]
async fn test_enforce_style() {
    let code = r#"fn MyFunction() {}
let MyVariable = 5;"#;

    let request = Request::new(proto::EnforceStyleRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        style_guide: "rust".to_string(),
        auto_fix: true,
    });

    let addr: SocketAddr = "127.0.0.1:50055".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.enforce_style(request).await.unwrap();
    
    assert!(response.into_inner().fixed_count > 0);
}

#[tokio::test]
async fn test_detect_errors() {
    let code = r#"fn risky() {
    let result = some_function().unwrap();
    todo!();
}"#;

    let request = Request::new(proto::DetectErrorsRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        language: "rust".to_string(),
    });

    let addr: SocketAddr = "127.0.0.1:50056".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.detect_errors(request).await.unwrap();
    
    assert!(response.into_inner().warning_count > 0);
}

#[tokio::test]
async fn test_log_error() {
    let request = Request::new(proto::LogErrorRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        error_code: "TEST_ERROR".to_string(),
        message: "Test error message".to_string(),
        stack_trace: "stack trace here".to_string(),
        context: "test context".to_string(),
        level: "ERROR".to_string(),
    });

    let addr: SocketAddr = "127.0.0.1:50057".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.log_error(request).await.unwrap();
    
    assert!(response.into_inner().success);
    assert!(!response.into_inner().error_id.is_empty());
}

#[tokio::test]
async fn test_get_logs() {
    let addr: SocketAddr = "127.0.0.1:50058".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    
    let log_request = Request::new(proto::LogErrorRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        error_code: "TEST".to_string(),
        message: "Test".to_string(),
        stack_trace: "".to_string(),
        context: "".to_string(),
        level: "INFO".to_string(),
    });
    client.client.log_error(log_request).await.unwrap();
    
    let request = Request::new(proto::GetLogsRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        level: "".to_string(),
        limit: 10,
        start_time: "".to_string(),
        end_time: "".to_string(),
    });
    
    let response = client.client.get_logs(request).await.unwrap();
    
    assert!(response.into_inner().count >= 1);
}

#[tokio::test]
async fn test_set_log_level() {
    let request = Request::new(proto::SetLogLevelRequest {
        level: "DEBUG".to_string(),
    });

    let addr: SocketAddr = "127.0.0.1:50059".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.set_log_level(request).await.unwrap();
    
    assert!(response.into_inner().success);
    assert_eq!(response.into_inner().current_level, "DEBUG");
}

#[tokio::test]
async fn test_go_to_type_definition() {
    let code = r#"struct Point {
    x: i32,
    y: i32,
}

fn main() {
    let p: Point = Point { x: 0, y: 0 };
}"#;

    let request = Request::new(proto::GoToTypeDefinitionRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        line: 7,
        character: 15,
    });

    let addr: SocketAddr = "127.0.0.1:50060".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.go_to_type_definition(request).await.unwrap();
    
    assert!(!response.into_inner().locations.is_empty());
}

#[tokio::test]
async fn test_find_implementations() {
    let code = r#"trait Shape {
    fn area(&self) -> f64;
}

struct Circle {
    radius: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }
}"#;

    let request = Request::new(proto::FindImplementationsRequest {
        session_id: "test_session".to_string(),
        tenant_id: "test_tenant".to_string(),
        file_path: "src/test.rs".to_string(),
        code: code.to_string(),
        symbol_name: "Shape".to_string(),
    });

    let addr: SocketAddr = "127.0.0.1:50061".parse().unwrap();
    tokio::spawn(async move {
        let builder = GrpcServerBuilder::new();
        let _ = builder.serve(addr).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    let mut client = TestClient::new(&addr).await;
    let response = client.client.find_implementations(request).await.unwrap();
    
    assert!(response.into_inner().count >= 1);
}