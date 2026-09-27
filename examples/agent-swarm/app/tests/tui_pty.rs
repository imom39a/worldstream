use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem as _};
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const SENTINEL: &str = "WORLDSTREAM_TUI_TERMINAL_USABLE";
const WAIT_LIMIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
enum FixtureCase {
    Interactive,
    InvalidStore,
    InputTermination,
}

struct PtyRun {
    output: String,
    success: bool,
}

#[test]
fn packaged_tui_handles_native_keys_unicode_paste_resize_and_restores_terminal()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut fixture = NativeFixture::spawn(directory.path(), FixtureCase::Interactive)?;

    fixture.wait_for("\u{1b}[?1049h")?;
    fixture.write(b"n")?;
    fixture.wait_for("GOAL:")?;
    fixture.write(unicode_entry())?;
    fixture.wait_for("Résumé")?;
    fixture.resize(72, 20)?;
    fixture.wait_for("72×20")?;
    fixture.write(b"!")?;
    fixture.wait_for("!")?;
    fixture.write(b"\x1b")?;
    fixture.wait_for("cancelled")?;
    fixture.write(b"q")?;

    let run = fixture.finish()?;
    assert!(run.success, "native TUI failed:\n{}", visible(&run.output));
    assert!(run.output.contains("GOAL:"), "{}", visible(&run.output));
    assert!(
        run.output.contains("Résumé") && run.output.contains('世') && run.output.contains('界'),
        "{}",
        visible(&run.output)
    );
    assert!(run.output.contains("72×20"), "{}", visible(&run.output));
    assert_restored_before_sentinel(&run.output)?;
    Ok(())
}

#[test]
fn packaged_tui_restores_after_backend_error_and_input_termination()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("invalid-store.json"), b"{")?;

    for case in [FixtureCase::InvalidStore, FixtureCase::InputTermination] {
        let fixture = NativeFixture::spawn(directory.path(), case)?;
        let run = fixture.finish()?;
        assert!(!run.success, "injected failure unexpectedly succeeded");
        match case {
            FixtureCase::InvalidStore => assert!(
                run.output.contains("backend data is invalid"),
                "{}",
                visible(&run.output)
            ),
            FixtureCase::InputTermination => {
                assert!(
                    run.output
                        .contains("injected native terminal input termination")
                );
            }
            FixtureCase::Interactive => unreachable!(),
        }
        assert_restored_before_sentinel(&run.output)?;
    }
    Ok(())
}

struct NativeFixture {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    output: Arc<Mutex<Vec<u8>>>,
    reader_done: mpsc::Receiver<std::io::Result<()>>,
}

impl NativeFixture {
    fn spawn(root: &Path, case: FixtureCase) -> Result<Self, Box<dyn std::error::Error>> {
        let pty = NativePtySystem::default().openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut command = wrapper_command();
        command.env("TERM", "xterm-256color");
        command.env("WORLDSTREAM_TUI_BIN", packaged_binary());
        command.env(
            "WORLDSTREAM_TUI_STATE",
            match case {
                FixtureCase::InvalidStore => root.join("invalid-store.json"),
                FixtureCase::Interactive | FixtureCase::InputTermination => {
                    root.join(format!("fixture-{}.json", case_name(case)))
                }
            },
        );
        command.env(
            "WORLDSTREAM_TUI_EXTRA",
            if matches!(case, FixtureCase::InputTermination) {
                "--inject-input-termination"
            } else {
                ""
            },
        );
        let reader = pty.master.try_clone_reader()?;
        let writer = pty.master.take_writer()?;
        let child = pty.slave.spawn_command(command)?;
        drop(pty.slave);

        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let (send, reader_done) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut reader = reader;
            let result = (|| {
                let mut chunk = [0_u8; 4096];
                loop {
                    let length = reader.read(&mut chunk)?;
                    if length == 0 {
                        return Ok(());
                    }
                    reader_output
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend_from_slice(&chunk[..length]);
                }
            })();
            let _ = send.send(result);
        });
        Ok(Self {
            master: pty.master,
            writer,
            child,
            output,
            reader_done,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), Box<dyn std::error::Error>> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    fn wait_for(&self, expected: &str) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + WAIT_LIMIT;
        loop {
            let captured = self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if String::from_utf8_lossy(&captured).contains(expected) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "native TUI did not render {expected:?}:\n{}",
                    visible(&String::from_utf8_lossy(&captured))
                )
                .into());
            }
            drop(captured);
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn finish(mut self) -> Result<PtyRun, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + WAIT_LIMIT;
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                return Err("native TUI PTY timed out".into());
            }
            thread::sleep(Duration::from_millis(20));
        };
        drop(self.writer);
        drop(self.master);
        self.reader_done.recv_timeout(WAIT_LIMIT)??;
        let bytes = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Ok(PtyRun {
            output: String::from_utf8_lossy(&bytes).into_owned(),
            success: status.success(),
        })
    }
}

fn packaged_binary() -> PathBuf {
    std::env::var_os("WORLDSTREAM_PACKAGED_AGENT_SWARM_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_worldstream-agent-swarm")),
        PathBuf::from,
    )
}

fn case_name(case: FixtureCase) -> &'static str {
    match case {
        FixtureCase::Interactive => "interactive",
        FixtureCase::InvalidStore => "invalid",
        FixtureCase::InputTermination => "termination",
    }
}

#[cfg(unix)]
fn unicode_entry() -> &'static [u8] {
    "\u{1b}[200~Résumé 世界\u{1b}[201~".as_bytes()
}

#[cfg(windows)]
fn unicode_entry() -> &'static [u8] {
    "Résumé 世界".as_bytes()
}

#[cfg(unix)]
fn wrapper_command() -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(
        r#"
normalize_stty() {
  state=$1
  lflag=${state#*lflag=}
  lflag=${lflag%%:*}
  masked=$(printf '%x' "$((0x$lflag & 0xdfffffff))")
  printf '%s' "$state" | sed "s/lflag=$lflag/lflag=$masked/"
}
before=$(normalize_stty "$(stty -g)") || exit 91
"$WORLDSTREAM_TUI_BIN" tui-native-fixture --state-file "$WORLDSTREAM_TUI_STATE" $WORLDSTREAM_TUI_EXTRA
status=$?
after=$(normalize_stty "$(stty -g)") || exit 92
if [ "$before" = "$after" ]; then
  printf '\nWORLDSTREAM_TUI_TERMINAL_USABLE\n'
else
  printf '\nWORLDSTREAM_TUI_TERMINAL_NOT_RESTORED before=%s after=%s\n' "$before" "$after"
  exit 93
fi
exit "$status"
"#,
    );
    command
}

#[cfg(windows)]
fn wrapper_command() -> CommandBuilder {
    let mut command = CommandBuilder::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        r#"$fixtureArgs=@('tui-native-fixture','--state-file',$env:WORLDSTREAM_TUI_STATE); if ($env:WORLDSTREAM_TUI_EXTRA) { $fixtureArgs += $env:WORLDSTREAM_TUI_EXTRA }; & $env:WORLDSTREAM_TUI_BIN @fixtureArgs; $status=$LASTEXITCODE; Write-Output 'WORLDSTREAM_TUI_TERMINAL_USABLE'; exit $status"#,
    ]);
    command
}

fn assert_restored_before_sentinel(output: &str) -> Result<(), Box<dyn std::error::Error>> {
    let leave = output
        .rfind("\u{1b}[?1049l")
        .ok_or_else(|| format!("alternate screen was not left:\n{}", visible(output)))?;
    let disable_paste = output
        .rfind("\u{1b}[?2004l")
        .ok_or_else(|| format!("bracketed paste was not disabled:\n{}", visible(output)))?;
    let sentinel = output
        .rfind(SENTINEL)
        .ok_or_else(|| format!("post-exit sentinel was not visible:\n{}", visible(output)))?;
    assert!(leave < sentinel);
    assert!(disable_paste < sentinel);
    assert!(!output.contains("WORLDSTREAM_TUI_TERMINAL_NOT_RESTORED"));
    Ok(())
}

fn visible(output: &str) -> String {
    output.replace('\u{1b}', "<ESC>")
}
