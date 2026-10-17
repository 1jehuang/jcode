use super::super::args::SessionCommand;
use super::*;

pub fn run_session_rename_command(
    session_ref: &str,
    name: Option<&str>,
    clear: bool,
    json: bool,
) -> Result<()> {
    let resolved_id = session::find_session_by_name_or_id(session_ref)?;
    let mut session = session::Session::load(&resolved_id)?;

    if clear {
        session.rename_title(None);
    } else {
        let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
            anyhow::bail!("Provide a session name or use --clear");
        };
        session.rename_title(Some(name.to_string()));
    }

    session.save()?;
    crate::tui::session_picker::invalidate_session_list_cache();

    let output = SessionRenameOutput {
        session_id: session.id.clone(),
        display_name: session.display_name().to_string(),
        title: session.display_title().map(ToOwned::to_owned),
        cleared: clear,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if clear {
        println!(
            "Cleared custom name for session {} ({}).",
            output.display_name, output.session_id
        );
    } else if let Some(title) = output.title.as_deref() {
        println!(
            "Renamed session {} ({}) to \"{}\".",
            output.display_name, output.session_id, title
        );
    }

    Ok(())
}

pub(crate) fn import_jcode_session_command(id: &str) -> Result<()> {
    let imported = crate::storage::session_import::import_jcode_session(id)?;
    println!(
        "Imported {} session files into KCode (jcode source unchanged)",
        imported.len()
    );
    Ok(())
}

pub(crate) fn run_session_command(command: SessionCommand) -> Result<()> {
    match command {
        SessionCommand::ImportJcode { session } => import_jcode_session_command(&session),
        SessionCommand::Rename {
            session,
            name,
            clear,
            json,
        } => run_session_rename_command(&session, name.as_deref(), clear, json),
    }
}
