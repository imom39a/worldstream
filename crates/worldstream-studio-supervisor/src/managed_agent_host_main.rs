use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use serde_json::{Value, json};
use zeroize::Zeroizing;

const MAX_MCP_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_CREDENTIAL_BYTES: usize = 16 * 1024;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(20);
const NO_WORK_BACKOFF: Duration = Duration::from_millis(250);

#[derive(Parser)]
#[command(name = "worldstream-managed-agent-host")]
struct Args {
    #[arg(long, value_parser = ["stdio"])]
    transport: String,
    #[arg(long, value_parser = ["openai-compatible"])]
    provider: String,
    #[arg(long)]
    provider_address: SocketAddr,
    #[arg(long)]
    model: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !args.provider_address.ip().is_loopback()
        || args.model.is_empty()
        || args.model.len() > 256
        || !args.model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        bail!("managed Agent Host configuration is invalid");
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = BufReader::new(stdin.lock());
    let credential = read_credential(&mut input).context("model credential delivery failed")?;
    let mut output = stdout.lock();
    let mut client = McpClient::initialize(&mut input, &mut output)
        .context("assignment MCP initialization failed")?;
    loop {
        match run_one_turn(&mut client, &args, &credential)
            .context("managed Activity turn failed")?
        {
            TurnOutcomeV1::Completed => {}
            TurnOutcomeV1::NoWork => std::thread::sleep(NO_WORK_BACKOFF),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TurnOutcomeV1 {
    Completed,
    NoWork,
}

enum ManagedTurnPreparationV1 {
    Prepared(Value),
    NoWork,
    RetryAfterTerminalLease,
}

enum ManagedTurnActionSubmissionV1 {
    Completed(Value),
    RetryAfterTerminalLease,
}

fn read_credential(input: &mut impl BufRead) -> Result<Zeroizing<Vec<u8>>> {
    let line = read_bounded_line(input, MAX_CREDENTIAL_BYTES.saturating_mul(2) + 10)?;
    let encoded = line
        .strip_prefix(b"WSMCRED1 ")
        .and_then(|value| value.strip_suffix(b"\n"))
        .context("credential frame is invalid")?;
    if encoded.is_empty() || encoded.len() % 2 != 0 || encoded.len() / 2 > MAX_CREDENTIAL_BYTES {
        bail!("credential frame is invalid");
    }
    let mut credential = Zeroizing::new(Vec::with_capacity(encoded.len() / 2));
    for pair in encoded.chunks_exact(2) {
        let text = std::str::from_utf8(pair).context("credential frame is invalid")?;
        credential.push(u8::from_str_radix(text, 16).context("credential frame is invalid")?);
    }
    Ok(credential)
}

fn run_one_turn(
    client: &mut McpClient<'_, impl BufRead, impl Write>,
    args: &Args,
    credential: &[u8],
) -> Result<TurnOutcomeV1> {
    let prepared = match client.prepare_managed_turn()? {
        ManagedTurnPreparationV1::Prepared(prepared) => prepared,
        ManagedTurnPreparationV1::NoWork => return Ok(TurnOutcomeV1::NoWork),
        ManagedTurnPreparationV1::RetryAfterTerminalLease => match client.prepare_managed_turn()? {
            ManagedTurnPreparationV1::Prepared(prepared) => prepared,
            ManagedTurnPreparationV1::NoWork => return Ok(TurnOutcomeV1::NoWork),
            ManagedTurnPreparationV1::RetryAfterTerminalLease => {
                bail!("assignment MCP repeated a terminal Activation lease failure")
            }
        },
    };
    if prepared.get("state").and_then(Value::as_str) == Some("reconciled") {
        return Ok(TurnOutcomeV1::Completed);
    }
    let observation = prepared
        .get("observation")
        .context("managed turn is invalid")?;
    let offers = prepared.get("offers").context("managed turn is invalid")?;
    let choice = request_model(args, credential, observation, offers)?;
    let offer_id = required_string(&choice, &["offer_id"])?;
    let payload = choice.get("payload").context("model choice is invalid")?;
    if !offers
        .get("offers")
        .and_then(Value::as_array)
        .is_some_and(|listed| {
            listed
                .iter()
                .any(|offer| offer.get("offer_id").and_then(Value::as_str) == Some(offer_id))
        })
    {
        bail!("model selected an unavailable Action offer");
    }
    let action_result = match client.submit_managed_turn_action(&json!({
        "offer_id": offer_id,
        "payload": payload,
    }))? {
        ManagedTurnActionSubmissionV1::Completed(result) => result,
        ManagedTurnActionSubmissionV1::RetryAfterTerminalLease => {
            return recover_terminal_submission(client);
        }
    };
    if action_result
        .get("action")
        .and_then(|action| action.get("status"))
        .and_then(Value::as_str)
        != Some("accepted")
    {
        bail!("assignment MCP Action was not accepted");
    }
    Ok(TurnOutcomeV1::Completed)
}

/// Recovers a terminal lease result returned after the model selected an Action.
///
/// The assignment helper owns the durable Action and Activation ledgers.  A
/// follow-up preparation may therefore reconcile the exact accepted Action on
/// a re-leased generation, but it must never ask the model to choose again.
fn recover_terminal_submission(
    client: &mut McpClient<'_, impl BufRead, impl Write>,
) -> Result<TurnOutcomeV1> {
    match client.prepare_managed_turn()? {
        ManagedTurnPreparationV1::Prepared(prepared)
            if prepared.get("state").and_then(Value::as_str) == Some("reconciled") =>
        {
            Ok(TurnOutcomeV1::Completed)
        }
        ManagedTurnPreparationV1::NoWork => Ok(TurnOutcomeV1::NoWork),
        ManagedTurnPreparationV1::Prepared(_) => {
            bail!("terminal lease recovery unexpectedly required a new model choice")
        }
        ManagedTurnPreparationV1::RetryAfterTerminalLease => {
            bail!("assignment MCP repeated a terminal Activation lease failure")
        }
    }
}

fn request_model(
    args: &Args,
    credential: &[u8],
    observation: &Value,
    offers: &Value,
) -> Result<Value> {
    let body = serde_json::to_vec(&json!({
        "model": args.model,
        "response_format": {"type": "json_object"},
        "messages": [{
            "role": "user",
            "content": serde_json::to_string(&json!({
                "instruction": "Select exactly one listed offer and return JSON with offer_id and payload.",
                "observation": observation,
                "offers": offers,
            }))?,
        }],
    }))?;
    if body.len() > MAX_MCP_MESSAGE_BYTES {
        bail!("model request is too large");
    }
    let mut authorization = Zeroizing::new(String::from("Bearer "));
    authorization.push_str(std::str::from_utf8(credential).context("model credential is invalid")?);
    if authorization
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        bail!("model credential is invalid");
    }
    let mut request = Zeroizing::new(format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        args.provider_address,
        authorization.as_str(),
        body.len()
    ));
    request.push_str(std::str::from_utf8(&body)?);
    let mut stream = TcpStream::connect_timeout(&args.provider_address, PROVIDER_TIMEOUT)
        .context("model provider is unavailable")?;
    stream.set_read_timeout(Some(PROVIDER_TIMEOUT))?;
    stream.set_write_timeout(Some(PROVIDER_TIMEOUT))?;
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    let response_deadline = Instant::now() + PROVIDER_TIMEOUT;
    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let remaining = response_deadline
            .checked_duration_since(Instant::now())
            .context("model provider response deadline exceeded")?;
        stream.set_read_timeout(Some(remaining))?;
        let read = stream
            .read(&mut buffer)
            .context("model provider response deadline exceeded")?;
        if read == 0 {
            break;
        }
        if response.len().saturating_add(read) > MAX_PROVIDER_RESPONSE_BYTES {
            bail!("model provider response is too large");
        }
        response.extend_from_slice(&buffer[..read]);
    }
    if response.len() > MAX_PROVIDER_RESPONSE_BYTES {
        bail!("model provider response is too large");
    }
    let boundary = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("model provider response is invalid")?;
    let header = std::str::from_utf8(&response[..boundary])?;
    if !header
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        bail!("model provider rejected the request");
    }
    let response: Value = serde_json::from_slice(&response[boundary + 4..])?;
    let content = required_string(&response, &["choices", "0", "message", "content"])?;
    let choice: Value = serde_json::from_str(content)?;
    if choice.as_object().is_none_or(|object| {
        object.len() != 2 || !object.contains_key("offer_id") || !object.contains_key("payload")
    }) {
        bail!("model choice is invalid");
    }
    Ok(choice)
}

struct McpClient<'a, R, W> {
    input: &'a mut R,
    output: &'a mut W,
    next_id: u64,
}

impl<'a, R: BufRead, W: Write> McpClient<'a, R, W> {
    fn initialize(input: &'a mut R, output: &'a mut W) -> Result<Self> {
        let mut client = Self {
            input,
            output,
            next_id: 1,
        };
        let initialized = client.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "worldstream-managed-agent-host", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        if initialized.get("protocolVersion").and_then(Value::as_str) != Some("2025-06-18") {
            bail!("assignment MCP initialization is invalid");
        }
        client.notify("notifications/initialized", &json!({}))?;
        Ok(client)
    }

    fn prepare_managed_turn(&mut self) -> Result<ManagedTurnPreparationV1> {
        let result = self.request(
            "tools/call",
            &json!({"name": "worldstream.prepare_managed_turn", "arguments": {}}),
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(false) {
            return result
                .get("structuredContent")
                .cloned()
                .map(ManagedTurnPreparationV1::Prepared)
                .context("assignment MCP response is invalid");
        }
        let content = result
            .get("structuredContent")
            .context("assignment MCP response is invalid")?;
        if content.get("code").and_then(Value::as_str)
            == Some("assignment_activation_none_available")
            && content.get("retryable").and_then(Value::as_bool) == Some(true)
            && content.get("next_action").and_then(Value::as_str)
                == Some("wait_then_request_next_activation")
        {
            return Ok(ManagedTurnPreparationV1::NoWork);
        }
        if terminal_lease_retry(content) {
            return Ok(ManagedTurnPreparationV1::RetryAfterTerminalLease);
        }
        bail!("assignment MCP tool returned a closed failure")
    }

    fn submit_managed_turn_action(
        &mut self,
        arguments: &Value,
    ) -> Result<ManagedTurnActionSubmissionV1> {
        let result = self.request(
            "tools/call",
            &json!({
                "name": "worldstream.submit_managed_turn_action",
                "arguments": arguments,
            }),
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(false) {
            return result
                .get("structuredContent")
                .cloned()
                .map(ManagedTurnActionSubmissionV1::Completed)
                .context("assignment MCP response is invalid");
        }
        let content = result
            .get("structuredContent")
            .context("assignment MCP response is invalid")?;
        if terminal_lease_retry(content) {
            return Ok(ManagedTurnActionSubmissionV1::RetryAfterTerminalLease);
        }
        bail!("assignment MCP tool returned a closed failure")
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("MCP request budget exhausted")?;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let line = read_bounded_line(self.input, MAX_MCP_MESSAGE_BYTES)?;
        let response: Value = serde_json::from_slice(&line)?;
        if response.get("id").and_then(Value::as_u64) != Some(id) || response.get("error").is_some()
        {
            bail!("assignment MCP response is invalid");
        }
        response
            .get("result")
            .cloned()
            .context("assignment MCP response is invalid")
    }

    fn notify(&mut self, method: &str, params: &Value) -> Result<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn write(&mut self, value: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_MCP_MESSAGE_BYTES {
            bail!("assignment MCP request is too large");
        }
        self.output.write_all(&bytes)?;
        self.output.write_all(b"\n")?;
        self.output.flush()?;
        Ok(())
    }
}

fn terminal_lease_retry(content: &Value) -> bool {
    matches!(
        content.get("code").and_then(Value::as_str),
        Some("assignment_activation_lease_expired" | "assignment_activation_cursor_stale")
    ) && content.get("retryable").and_then(Value::as_bool) == Some(true)
        && content.get("next_action").and_then(Value::as_str) == Some("request_next_activation")
}

fn read_bounded_line(input: &mut impl BufRead, maximum: usize) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    input
        .take(u64::try_from(maximum + 1).unwrap_or(u64::MAX))
        .read_until(b'\n', &mut line)?;
    if line.is_empty() || line.len() > maximum || !line.ends_with(b"\n") {
        bail!("bounded line input is invalid");
    }
    Ok(line)
}

fn required_string<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    descend(value, path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_MCP_MESSAGE_BYTES)
        .context("bounded response field is invalid")
}

fn descend<'a>(mut value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for segment in path {
        value = if let Ok(index) = segment.parse::<usize>() {
            value.as_array()?.get(index)?
        } else {
            value.get(*segment)?
        };
    }
    Some(value)
}
