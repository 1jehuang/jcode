use super::*;

#[tokio::test]
async fn catalog_request_headers_follow_provider_auth_rules() {
    for base in [
        "https://api.anthropic.com/v1",
        "https://API.ANTHROPIC.COM/v1",
    ] {
        for auth in [
            ProviderAuth::InlineBearer {
                token: "test-key".into(),
                label: "test".into(),
            },
            ProviderAuth::InlineHeader {
                header_name: HeaderName::from_static("x-api-key"),
                token: "test-key".into(),
                label: "test".into(),
            },
        ] {
            let request = auth
                .apply_catalog(Client::new().get(format!("{base}/models")), base)
                .await
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(request.method(), reqwest::Method::GET);
            assert_eq!(request.url().path(), "/v1/models");
            assert_eq!(request.headers()["anthropic-version"], "2023-06-01");
            assert_eq!(request.headers()["x-api-key"], "test-key");
            assert!(request.headers().get("authorization").is_none());
            assert_eq!(request.headers().get_all("x-api-key").iter().count(), 1);
        }
    }

    for base in [
        "https://openrouter.ai/api/v1",
        "https://api.openai.com/v1",
        "http://localhost:11434/v1",
    ] {
        for (auth, expected_header, expected_value) in [
            (
                ProviderAuth::InlineBearer {
                    token: "test-key".into(),
                    label: "test".into(),
                },
                Some("authorization"),
                "Bearer test-key",
            ),
            (
                ProviderAuth::InlineHeader {
                    header_name: HeaderName::from_static("api-key"),
                    token: "test-key".into(),
                    label: "test".into(),
                },
                Some("api-key"),
                "test-key",
            ),
            (
                ProviderAuth::None {
                    label: "test".into(),
                },
                None,
                "",
            ),
        ] {
            let request = auth
                .apply_catalog(Client::new().get(format!("{base}/models")), base)
                .await
                .unwrap()
                .build()
                .unwrap();
            assert!(request.headers().get("anthropic-version").is_none());
            assert!(request.headers().get("x-api-key").is_none());
            if let Some(header) = expected_header {
                assert_eq!(request.headers()[header], expected_value);
            } else {
                assert!(request.headers().is_empty());
            }
        }
    }
}
