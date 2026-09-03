//! Exact one-use client handoffs reach the browser through protected local files.

use super::cli_contract::ClientOpenArgs;
use serde::Serialize;
use std::{
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_data_directory,
};
use worldstream_studio_supervisor::{
    operator_connection::OperatorConnection,
    participant_handoff::{
        ClientCandidateSummaryV1, ClientHandoffResponseV1, OperatorClientHandoffRequestV1,
    },
};
use zeroize::Zeroizing;

#[derive(Debug, Serialize)]
pub struct OpenedClient {
    pub operation: String,
    pub seat: String,
    pub redirect_file: PathBuf,
}

#[derive(Debug)]
pub enum ClientOpenExecution {
    Opened(OpenedClient),
    SelectionRequired(Vec<ClientCandidateSummaryV1>),
    Rejected,
    Failed,
    Unavailable,
}

/// Only an already-protected local filename crosses the operating-system boundary.
trait BrowserOpener {
    fn open(&self, path: &Path) -> io::Result<()>;
}

struct SystemBrowser;
impl BrowserOpener for SystemBrowser {
    fn open(&self, path: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        let program = "/usr/bin/open";
        #[cfg(target_os = "windows")]
        let program = "explorer.exe";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let program = "xdg-open";
        let status = Command::new(program)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("browser could not be opened"))
        }
    }
}

pub fn execute(arguments: &ClientOpenArgs) -> ClientOpenExecution {
    let selection = &arguments.seat;
    let options = &selection.options;
    let Ok(connection) = OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(u64::from(options.timeout_seconds)),
    ) else {
        return ClientOpenExecution::Unavailable;
    };
    let Ok(body) = serde_json::to_vec(&OperatorClientHandoffRequestV1 {
        binding: arguments.binding.clone(),
    }) else {
        return ClientOpenExecution::Rejected;
    };
    let path = format!(
        "/api/v1/room-setup-operations/{}/seats/{}/client-handoff",
        selection.operation, selection.seat
    );
    let Ok(response) = connection.request("POST", &path, &body) else {
        return ClientOpenExecution::Unavailable;
    };
    let bytes = Zeroizing::new(response.body);
    if matches!(response.status, 400 | 404 | 409) {
        return ClientOpenExecution::Rejected;
    }
    if !matches!(response.status, 200 | 201) {
        return ClientOpenExecution::Unavailable;
    }
    let Ok(result) = serde_json::from_slice::<ClientHandoffResponseV1>(&bytes) else {
        return ClientOpenExecution::Unavailable;
    };
    match result {
        ClientHandoffResponseV1::SelectionRequired {
            version,
            candidates,
        } => {
            if version != "activity_client_handoff.v1" {
                return ClientOpenExecution::Unavailable;
            }
            ClientOpenExecution::SelectionRequired(candidates)
        }
        ClientHandoffResponseV1::Ready {
            version,
            client_url,
        } => {
            let url = Zeroizing::new(client_url);
            if version != "activity_client_handoff.v1" {
                return ClientOpenExecution::Unavailable;
            }
            match open_handoff(&options.state_dir, &url, &SystemBrowser) {
                Ok(redirect_file) => ClientOpenExecution::Opened(OpenedClient {
                    operation: selection.operation.clone(),
                    seat: selection.seat.clone(),
                    redirect_file,
                }),
                Err(_) => ClientOpenExecution::Failed,
            }
        }
    }
}

fn open_handoff(state: &Path, url: &str, opener: &impl BrowserOpener) -> io::Result<PathBuf> {
    if url.len() > 8192
        || url.chars().any(char::is_control)
        || !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return Err(io::Error::other("invalid client handoff"));
    }
    let state =
        validate_data_directory(state).map_err(|_| io::Error::other("unsafe handoff directory"))?;
    let directory = prepare_data_directory(&state.join("client-handoffs"))
        .map_err(|_| io::Error::other("unsafe handoff directory"))?;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| io::Error::other("handoff identity unavailable"))?;
    let mut filename = String::from("handoff-");
    for byte in random {
        let _ = write!(&mut filename, "{byte:02x}");
    }
    filename.push_str(".html");
    let path = directory.join(filename);
    let escaped = Zeroizing::new(
        url.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "&#39;"),
    );
    let document = Zeroizing::new(format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><meta http-equiv=\"refresh\" content=\"0;url={}\"><title>Opening Activity Client</title>",
        escaped.as_str()
    ));
    let mut file =
        create_owner_only_file(&path).map_err(|_| io::Error::other("handoff file unavailable"))?;
    if file
        .write_all(document.as_bytes())
        .and_then(|()| file.sync_all())
        .is_err()
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(io::Error::other("handoff file incomplete"));
    }
    drop(file);
    opener.open(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{BrowserOpener, open_handoff};
    use std::{
        cell::RefCell,
        io,
        path::{Path, PathBuf},
    };

    struct RecordingOpener(RefCell<Option<PathBuf>>);

    impl BrowserOpener for RecordingOpener {
        fn open(&self, path: &Path) -> io::Result<()> {
            worldstream_runtime::validate_owner_only_file(path)
                .map_err(|_| io::Error::other("opener must receive an already-protected file"))?;
            assert!(!path.to_string_lossy().contains("HANDOFF_CANARY"));
            *self.0.borrow_mut() = Some(path.to_path_buf());
            Ok(())
        }
    }

    #[test]
    fn browser_boundary_receives_only_protected_file_not_handoff_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let state = worldstream_runtime::prepare_data_directory(&directory.path().join("state"))?;
        let opener = RecordingOpener(RefCell::new(None));
        let path = open_handoff(
            &state,
            "http://127.0.0.1:9400/client?handoff=HANDOFF_CANARY&mode=participant",
            &opener,
        )?;
        assert_eq!(opener.0.into_inner(), Some(path.clone()));
        let html = std::fs::read_to_string(path)?;
        assert!(html.contains("handoff=HANDOFF_CANARY&amp;mode=participant"));
        assert!(html.contains("no-referrer"));
        Ok(())
    }
}
