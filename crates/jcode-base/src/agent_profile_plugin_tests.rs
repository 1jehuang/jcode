use super::*;
use crate::plugin::PluginScope;
use crate::plugin_components::fixture::Fixture;

#[test]
fn plugin_profiles_list_load_scope_disable_and_uninstall() {
    let f = Fixture::new();
    f.manager
        .install("registry-fixture", PluginScope::Local)
        .unwrap();
    let names = list(&f.project).unwrap();
    assert!(names.contains(&"registry-fixture:reviewer".to_owned()));
    assert!(names.contains(&"registry-fixture:custom".to_owned()));
    assert!(
        !list(&f.other)
            .unwrap()
            .contains(&"registry-fixture:reviewer".to_owned())
    );
    let (profile, fields) = load_with_metadata("registry-fixture:reviewer", &f.project).unwrap();
    assert_eq!(profile.name, "registry-fixture:reviewer");
    assert_eq!(profile.instructions.trim(), "Review carefully.");
    assert_eq!(fields, ["model"]);
    assert!(profile.instruction_sources.is_empty());
    assert!(load("registry-fixture:../reviewer", &f.project).is_err());
    f.manager
        .disable("registry-fixture", PluginScope::Local)
        .unwrap();
    assert!(!list(&f.project).unwrap().contains(&profile.name));
    assert!(load(&profile.name, &f.project).is_err());
    f.manager
        .enable("registry-fixture", PluginScope::Local)
        .unwrap();
    assert!(load(&profile.name, &f.project).is_ok());
    f.manager
        .uninstall("registry-fixture", PluginScope::Local)
        .unwrap();
    assert!(load(&profile.name, &f.project).is_err());
    assert!(!list(&f.project).unwrap().contains(&profile.name));
}
