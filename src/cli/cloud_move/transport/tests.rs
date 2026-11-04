use super::*;

fn scripted_target(script: &str) -> Result<(tempfile::TempDir, Target)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("transport.sh");
    std::fs::write(&path, script)?;
    let target = Target {
        transport: Some(format!("/bin/sh {}", path.display())),
        ..Target::default()
    };
    Ok((dir, target))
}

fn account() -> CloudAccount {
    CloudAccount {
        provider: transfer::TransferProvider::OpenAi,
        label: "test-account".to_string(),
    }
}

#[test]
fn custom_transport_preserves_remote_argument_quoting() -> Result<()> {
    let target = Target {
        transport: Some("/bin/sh -c".to_string()),
        remote_binary: Some("/a directory/jcode".to_string()),
        ..Target::default()
    };
    let command = target.remote_command("unused", &["cloud", "a'b", "two words"])?;
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args.len(), 2);
    let script = args[1].to_string_lossy();
    assert!(script.contains(&shell_quote("/a directory/jcode")));
    assert!(script.ends_with(&format!(
        "{} {} {}",
        shell_quote("cloud"),
        shell_quote("a'b"),
        shell_quote("two words")
    )));
    Ok(())
}

#[test]
fn empty_custom_transport_is_an_error() {
    let target = Target {
        transport: Some("  ".to_string()),
        ..Target::default()
    };
    assert!(target.remote_command("unused", &["cloud"]).is_err());
}

#[test]
fn ssh_drop_is_distinct_from_remote_command_failure() -> Result<()> {
    let (_drop_dir, dropped) = scripted_target("exit 255\n")?;
    let drop_error = dropped
        .run_remote("unused", &["cloud", "receive"], None)
        .expect_err("SSH drop must fail");
    assert!(drop_error.downcast_ref::<SshDropped>().is_some());
    let (_fail_dir, failed) = scripted_target("printf 'remote failure' >&2\nexit 42\n")?;
    let mut attempts = 0;
    let result = retry_ssh("unused", "test", || {
        attempts += 1;
        failed.run_remote("unused", &["cloud", "receive"], None)
    });
    let error = result.expect_err("remote command failure must fail");
    assert!(error.downcast_ref::<SshDropped>().is_none());
    assert!(error.to_string().contains("remote failure"));
    assert_eq!(attempts, 1);
    Ok(())
}

#[test]
fn credential_import_rejects_unexpected_output_without_disclosing_it() -> Result<()> {
    let (_dir, target) = scripted_target("cat >/dev/null\nprintf 'private response'\n")?;
    let error = target
        .exchange_credentials("unused", &account(), Some(b"credential"))
        .expect_err("imports must not produce output");
    assert!(
        error
            .to_string()
            .contains("unexpected cloud credential import response")
    );
    assert!(!format!("{error:#}").contains("private response"));
    Ok(())
}

#[test]
fn credential_export_returns_private_bytes() -> Result<()> {
    let (_dir, target) = scripted_target("printf 'private response'\n")?;
    let bytes = target.exchange_credentials("unused", &account(), None)?;
    assert_eq!(bytes, b"private response");
    Ok(())
}

#[test]
fn credential_failures_do_not_echo_remote_secrets() -> Result<()> {
    let (_dir, target) =
        scripted_target("cat >/dev/null\nprintf 'private failure' >&2\nexit 42\n")?;
    let credential_error = target
        .exchange_credentials("unused", &account(), Some(b"credential"))
        .expect_err("failed credential import must fail");
    let router_error = target
        .send_router_credentials("unused", b"credential")
        .expect_err("failed router import must fail");
    assert!(!format!("{credential_error:#}").contains("private failure"));
    assert!(!format!("{router_error:#}").contains("private failure"));
    Ok(())
}
