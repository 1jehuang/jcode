use jcode_applet_types::view::{NodeKind, host_action};
use jcode_applet_types::*;
use serde_json::json;

fn manifest(capabilities: &[Capability]) -> Manifest {
    serde_json::from_value(json!({
        "schema": SCHEMA,
        "id": "test.applet",
        "title": "Test",
        "capabilities": capabilities,
    }))
    .unwrap()
}

fn document(view: serde_json::Value) -> Document {
    serde_json::from_value(json!({"revision": 1, "title": "T", "view": view})).unwrap()
}

#[test]
fn round_trips_a_rich_document_with_images_and_placements() {
    let message: ProviderMessage = serde_json::from_value(json!({
        "type": "mount",
        "instance": "i1",
        "placement": {"kind": "overlay", "corner": "top_right"},
        "lifetime": "persistent",
        "document": {
            "revision": 0,
            "title": "Photos",
            "state": {"q": ""},
            "assets": [{"id": "cat", "mime": "image/png", "data": "iVBORw0KGgo="}],
            "view": {"type": "stack", "children": [
                {"type": "input", "bind": "q", "placeholder": "Search", "on_submit": {"action": "search"}},
                {"type": "grid", "min_column_width": 120, "children": [
                    {"type": "image", "key": "a", "source": {"asset": "cat"}, "alt": "Cat", "aspect_ratio": 1.5, "fit": "cover"},
                    {"type": "image", "key": "b", "source": {"data": "data:image/png;base64,iVBORw0KGgo="}, "alt": "Dot"}
                ]},
                {"type": "list", "children": [
                    {"type": "list_item", "key": "m1", "title": "Hello", "badges": ["unread"],
                     "leading": {"type": "image", "source": {"asset": "cat"}, "alt": "", "shape": "circle"},
                     "on_press": {"action": "open", "args": {"id": "m1"}}}
                ]}
            ]}
        }
    }))
    .unwrap();
    let text = serde_json::to_string(&message).unwrap();
    let again: ProviderMessage = serde_json::from_str(&text).unwrap();
    assert_eq!(message, again);
    let ProviderMessage::Mount {
        document,
        placement,
        ..
    } = message
    else {
        panic!("expected mount");
    };
    assert!(matches!(placement, Placement::Overlay { .. }));
    validate_document(&document, &manifest(&[]), &Limits::default()).unwrap();
}

#[test]
fn placements_are_independent_of_tool_calls() {
    for placement in [
        json!({"kind": "panel"}),
        json!({"kind": "sidebar"}),
        json!({"kind": "composer", "session_id": "s"}),
        json!({"kind": "background"}),
        json!({"kind": "inline", "session_id": "s", "anchor": {"kind": "end"}}),
        json!({"kind": "inline", "session_id": "s", "anchor": {"kind": "after_message", "message_id": "m"}}),
        json!({"kind": "inline", "session_id": "s", "anchor": {"kind": "tool_call", "call_id": "c"}}),
    ] {
        let parsed: Placement = serde_json::from_value(placement.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["kind"],
            placement["kind"]
        );
    }
}

#[test]
fn unknown_nodes_degrade_to_fallback_and_survive_round_trip() {
    let doc = document(json!({"type": "stack", "children": [
        {"type": "map_view", "fallback": "Map of Paris", "lat": 48.8}
    ]}));
    let child = &doc.view.children()[0];
    assert!(matches!(&child.kind, NodeKind::Unknown { type_name, .. } if type_name == "map_view"));
    assert_eq!(child.fallback.as_deref(), Some("Map of Paris"));
    let relayed = serde_json::to_value(child).unwrap();
    assert_eq!(
        relayed,
        json!({"type": "map_view", "fallback": "Map of Paris", "lat": 48.8})
    );
    validate_document(&doc, &manifest(&[]), &Limits::default()).unwrap();
}

#[test]
fn malformed_known_nodes_are_errors() {
    let error = serde_json::from_value::<Document>(json!({
        "revision": 1, "title": "T", "view": {"type": "button", "label": "No action"}
    }))
    .unwrap_err();
    assert!(
        error.to_string().contains("invalid `button` node"),
        "{error}"
    );
}

#[test]
fn capabilities_gate_host_actions_and_image_sources() {
    let open = document(json!({"type": "button", "label": "Open",
        "on_press": {"action": host_action::OPEN_URL, "args": {"url": "https://x.dev"}}}));
    assert_eq!(
        validate_document(&open, &manifest(&[]), &Limits::default()),
        Err(ValidationError::MissingCapability(Capability::OpenUrl))
    );
    validate_document(&open, &manifest(&[Capability::OpenUrl]), &Limits::default()).unwrap();

    let js = document(json!({"type": "button", "label": "x",
        "on_press": {"action": host_action::OPEN_URL, "args": {"url": "javascript:alert(1)"}}}));
    assert!(validate_document(&js, &manifest(&[Capability::OpenUrl]), &Limits::default()).is_err());

    let remote =
        document(json!({"type": "image", "source": {"url": "https://x.dev/a.png"}, "alt": "a"}));
    assert_eq!(
        validate_document(&remote, &manifest(&[]), &Limits::default()),
        Err(ValidationError::MissingCapability(Capability::RemoteImages))
    );
    let insecure =
        document(json!({"type": "image", "source": {"url": "http://x.dev/a.png"}, "alt": "a"}));
    assert!(
        validate_document(
            &insecure,
            &manifest(&[Capability::RemoteImages]),
            &Limits::default()
        )
        .is_err()
    );

    let relative = document(json!({"type": "image", "source": {"path": "a.png"}, "alt": "a"}));
    assert!(
        validate_document(
            &relative,
            &manifest(&[Capability::ReadFiles]),
            &Limits::default()
        )
        .is_err()
    );

    let html = document(json!({"type": "html", "source": "<b>x</b>", "height": 100}));
    assert_eq!(
        validate_document(&html, &manifest(&[]), &Limits::default()),
        Err(ValidationError::MissingCapability(Capability::Html))
    );

    let unknown_host = document(
        json!({"type": "button", "label": "x", "on_press": {"action": "host.format_disk"}}),
    );
    assert!(validate_document(&unknown_host, &manifest(&[]), &Limits::default()).is_err());
}

#[test]
fn enforces_limits_and_references() {
    let missing = document(json!({"type": "image", "source": {"asset": "nope"}, "alt": "a"}));
    assert_eq!(
        validate_document(&missing, &manifest(&[]), &Limits::default()),
        Err(ValidationError::UnknownAsset("nope".into()))
    );

    let dup = document(json!({"type": "stack", "children": [
        {"type": "spacer", "key": "k"}, {"type": "divider", "key": "k"}
    ]}));
    assert_eq!(
        validate_document(&dup, &manifest(&[]), &Limits::default()),
        Err(ValidationError::DuplicateKey("k".into()))
    );

    let mut deep = json!({"type": "spacer"});
    for _ in 0..30 {
        deep = json!({"type": "stack", "children": [deep]});
    }
    assert!(matches!(
        validate_document(&document(deep), &manifest(&[]), &Limits::default()),
        Err(ValidationError::TooDeep(_))
    ));

    let big = format!("data:image/png;base64,{}", "A".repeat(400_000));
    let inline = document(json!({"type": "image", "source": {"data": big}, "alt": "a"}));
    assert!(matches!(
        validate_document(&inline, &manifest(&[]), &Limits::default()),
        Err(ValidationError::InlineImageTooLarge(_))
    ));

    let ragged = document(json!({"type": "table", "columns": ["a", "b"], "rows": [["1"]]}));
    assert!(matches!(
        validate_document(&ragged, &manifest(&[]), &Limits::default()),
        Err(ValidationError::RaggedTable { .. })
    ));

    let mut bad_mime = document(json!({"type": "spacer"}));
    bad_mime.assets.push(AssetDecl {
        id: "x".into(),
        mime: "text/html".into(),
        data: String::new(),
    });
    assert!(matches!(
        validate_document(&bad_mime, &manifest(&[]), &Limits::default()),
        Err(ValidationError::UnsupportedMime { .. })
    ));
}

#[test]
fn patches_are_revisioned_atomic_and_path_restricted() {
    let doc = document(json!({"type": "stack", "children": [
        {"type": "text", "text": "a"}
    ]}));
    let next = apply_patch(
        &doc,
        1,
        &[
            PatchOp::Replace {
                path: "/view/children/0/text".into(),
                value: json!("b"),
            },
            PatchOp::Add {
                path: "/view/children/-".into(),
                value: json!({"type": "divider"}),
            },
            PatchOp::Add {
                path: "/state/count".into(),
                value: json!(3),
            },
        ],
    )
    .unwrap();
    assert_eq!(next.revision, 2);
    assert_eq!(next.view.children().len(), 2);
    assert_eq!(next.state["count"], 3);

    assert_eq!(
        apply_patch(&doc, 0, &[]),
        Err(PatchError::RevisionMismatch {
            expected: 1,
            got: 0
        })
    );
    assert!(matches!(
        apply_patch(
            &doc,
            1,
            &[PatchOp::Replace {
                path: "/revision".into(),
                value: json!(9)
            }]
        ),
        Err(PatchError::ForbiddenPath(_))
    ));
    assert!(matches!(
        apply_patch(
            &doc,
            1,
            &[PatchOp::Remove {
                path: "/view/children/5".into()
            }]
        ),
        Err(PatchError::PathNotFound(_))
    ));
    // A patch producing an invalid known node is rejected as a whole.
    assert!(matches!(
        apply_patch(
            &doc,
            1,
            &[
                PatchOp::Replace {
                    path: "/view/children/0/text".into(),
                    value: json!("ok")
                },
                PatchOp::Replace {
                    path: "/view/children/0/type".into(),
                    value: json!("button")
                },
            ]
        ),
        Err(PatchError::InvalidDocument(_))
    ));
}

#[test]
fn manifest_schema_and_tool_card_claims() {
    let m: Manifest = serde_json::from_value(json!({
        "schema": "jcode.applet/1",
        "id": "mail",
        "title": "Mail",
        "launchers": [
            {"trigger": "sidebar", "placement": {"kind": "panel"}},
            {"trigger": "shortcut", "keys": "super-shift-g", "placement": {"kind": "panel"}},
            {"trigger": "startup", "placement": {"kind": "sidebar"}, "singleton": false}
        ],
        "tool_cards": [{"tool": "gmail", "actions": ["read", "thread"]}]
    }))
    .unwrap();
    validate::validate_manifest(&m).unwrap();
    assert!(m.launchers[0].singleton);
    assert!(!m.launchers[2].singleton);
    assert!(m.tool_cards[0].matches("gmail", Some("read")));
    assert!(!m.tool_cards[0].matches("gmail", Some("send")));
    assert!(!m.tool_cards[0].matches("gmail", None));

    let mut future = m.clone();
    future.schema = "jcode.applet/2".into();
    assert!(validate::validate_manifest(&future).is_err());
}
