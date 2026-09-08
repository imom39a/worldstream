use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use serde_json::{Value, json};
use worldstream_hosted_contract::HouseAgentRevision;
use worldstream_studio_supervisor::house_model::{
    DevelopmentLoopbackOpenRouterProviderPortV1, FileHouseAllowanceLedgerV1,
    HouseAllowancePeriodV1, HouseInvocationIdentityV1, HouseModelExecutorV1,
    HouseProviderCredentialV1, HouseProviderPortV1, HouseSpendLimitsV1, OpenRouterProviderPortV1,
};
use zeroize::Zeroizing;

const MAX_MCP_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_CREDENTIAL_BYTES: usize = 16 * 1024;
const MAX_HOUSE_REVISION_BYTES: usize = 256 * 1024;
// Every House host starts in `<house-root>/units/<stable-unit>`. This fixed
// sibling path preserves a private per-unit working directory while all units
// share the one process-locked deployment spend ledger.
const HOUSE_ALLOWANCE_LEDGER_DIRECTORY: &str = "../allowance";
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(20);
const NO_WORK_BACKOFF: Duration = Duration::from_millis(250);

#[derive(Parser)]
#[command(name = "worldstream-managed-agent-host")]
struct Args {
    #[arg(long, value_parser = ["stdio"])]
    transport: String,
    #[arg(long, value_parser = ["openai-compatible", "openrouter-house"])]
    provider: String,
    #[arg(long)]
    provider_address: Option<SocketAddr>,
    #[arg(long)]
    model: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = BufReader::new(stdin.lock());
    let mut output = stdout.lock();
    match args.provider.as_str() {
        "openai-compatible" => {
            let provider_address = args
                .provider_address
                .filter(|address| address.ip().is_loopback())
                .context("managed Agent Host configuration is invalid")?;
            let model = args
                .model
                .filter(|model| valid_model(model))
                .context("managed Agent Host configuration is invalid")?;
            let credential =
                read_credential(&mut input).context("model credential delivery failed")?;
            let mut client = McpClient::initialize(&mut input, &mut output)
                .context("assignment MCP initialization failed")?;
            loop {
                match run_one_reference_turn(&mut client, provider_address, &model, &credential)
                    .context("managed Activity turn failed")?
                {
                    TurnOutcomeV1::Completed => {}
                    TurnOutcomeV1::NoWork => std::thread::sleep(NO_WORK_BACKOFF),
                }
            }
        }
        "openrouter-house" => {
            if args.model.is_some() {
                bail!("managed Agent Host configuration is invalid");
            }
            let startup =
                read_house_startup(&mut input).context("House configuration delivery failed")?;
            if let Some(address) = args.provider_address {
                let provider = DevelopmentLoopbackOpenRouterProviderPortV1::new(address)
                    .context("managed Agent Host configuration is invalid")?;
                run_house_host(&mut input, &mut output, &startup, provider)
            } else {
                run_house_host(
                    &mut input,
                    &mut output,
                    &startup,
                    OpenRouterProviderPortV1::new(),
                )
            }
        }
        _ => bail!("managed Agent Host configuration is invalid"),
    }
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
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
    OfferUnavailable,
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

struct HouseStartupV1 {
    runner_unit_id: String,
    revision: HouseAgentRevision,
    credential: HouseProviderCredentialV1,
}

fn run_house_host<P: HouseProviderPortV1>(
    input: &mut impl BufRead,
    output: &mut impl Write,
    startup: &HouseStartupV1,
    provider: P,
) -> Result<()> {
    let ledger = FileHouseAllowanceLedgerV1::open_for_assignment(
        HOUSE_ALLOWANCE_LEDGER_DIRECTORY,
        HouseSpendLimitsV1::hobby_preview(),
        &startup.runner_unit_id,
    )
    .context("House allowance ledger is unavailable")?;
    let executor = HouseModelExecutorV1::new(provider, ledger);
    let mut client =
        McpClient::initialize(input, output).context("assignment MCP initialization failed")?;
    loop {
        match run_one_house_turn(
            &mut client,
            &startup.runner_unit_id,
            &startup.revision,
            &startup.credential,
            &executor,
        )
        .context("managed House turn failed")?
        {
            TurnOutcomeV1::Completed => {}
            TurnOutcomeV1::NoWork => std::thread::sleep(NO_WORK_BACKOFF),
        }
    }
}

fn read_house_startup(input: &mut impl BufRead) -> Result<HouseStartupV1> {
    let line = read_bounded_line(input, 512)?;
    let header = std::str::from_utf8(&line)?
        .strip_suffix('\n')
        .context("House startup frame is invalid")?;
    let mut fields = header.split_ascii_whitespace();
    if fields.next() != Some("WSMHOUSE1") {
        bail!("House startup frame is invalid");
    }
    let runner_unit_id = fields
        .next()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.as_bytes()[0].is_ascii_alphanumeric()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .context("House startup frame is invalid")?
        .to_owned();
    let revision_length = fields
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=MAX_HOUSE_REVISION_BYTES).contains(value))
        .context("House startup frame is invalid")?;
    let credential_length = fields
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (32..=512).contains(value))
        .context("House startup frame is invalid")?;
    if fields.next().is_some() {
        bail!("House startup frame is invalid");
    }
    let mut revision_bytes = Zeroizing::new(vec![0_u8; revision_length]);
    input.read_exact(&mut revision_bytes)?;
    let revision = HouseAgentRevision::from_canonical_bytes(&revision_bytes)
        .context("House startup revision is invalid")?;
    let mut credential = Zeroizing::new(vec![0_u8; credential_length]);
    input.read_exact(&mut credential)?;
    let credential = HouseProviderCredentialV1::new(credential)
        .context("House provider credential is invalid")?;
    Ok(HouseStartupV1 {
        runner_unit_id,
        revision,
        credential,
    })
}

fn run_one_reference_turn(
    client: &mut McpClient<'_, impl BufRead, impl Write>,
    provider_address: SocketAddr,
    model: &str,
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
    let choice = request_model(provider_address, model, credential, observation, offers)?;
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
        ManagedTurnActionSubmissionV1::OfferUnavailable => {
            client.fail_managed_turn()?;
            return Ok(TurnOutcomeV1::Completed);
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

fn run_one_house_turn<P>(
    client: &mut McpClient<'_, impl BufRead, impl Write>,
    runner_unit_id: &str,
    revision: &HouseAgentRevision,
    credential: &HouseProviderCredentialV1,
    executor: &HouseModelExecutorV1<P>,
) -> Result<TurnOutcomeV1>
where
    P: worldstream_studio_supervisor::house_model::HouseProviderPortV1,
{
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
    let provider_attempt = prepared
        .get("provider_attempt")
        .context("managed House turn is invalid")?;
    if provider_attempt.get("schema").and_then(Value::as_str)
        != Some("worldstream/house-provider-attempt/v1")
    {
        bail!("managed House turn is invalid");
    }
    let identity = HouseInvocationIdentityV1::from_sealed_attempt(
        runner_unit_id,
        required_string(provider_attempt, &["digest"])?,
    )
    .context("managed House turn is invalid")?;
    let observation = prepared
        .get("observation")
        .context("managed House turn is invalid")?;
    let offers = prepared
        .get("offers")
        .context("managed House turn is invalid")?;
    let period = HouseAllowancePeriodV1::now().context("House allowance clock is unavailable")?;
    let completion =
        match executor.execute(credential, revision, &identity, observation, offers, period) {
            Ok(completion) => completion,
            Err(_closed_failure) => {
                client
                    .fail_managed_turn()
                    .context("assignment MCP could not close the failed House turn")?;
                return Ok(TurnOutcomeV1::Completed);
            }
        };
    let action_result = match client.submit_managed_turn_action(&json!({
        "offer_id": completion.action.offer_id,
        "payload": completion.action.payload,
    }))? {
        ManagedTurnActionSubmissionV1::Completed(result) => result,
        ManagedTurnActionSubmissionV1::RetryAfterTerminalLease => {
            return recover_terminal_submission(client);
        }
        ManagedTurnActionSubmissionV1::OfferUnavailable => {
            // Another participant or timer may advance the Room during model
            // execution. Close this turn; never repeat its paid provider call.
            client.fail_managed_turn()?;
            return Ok(TurnOutcomeV1::Completed);
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
    provider_address: SocketAddr,
    model: &str,
    credential: &[u8],
    observation: &Value,
    offers: &Value,
) -> Result<Value> {
    let body = serde_json::to_vec(&json!({
        "model": model,
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
        provider_address,
        authorization.as_str(),
        body.len()
    ));
    request.push_str(std::str::from_utf8(&body)?);
    let mut stream = TcpStream::connect_timeout(&provider_address, PROVIDER_TIMEOUT)
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
        if content.get("code").and_then(Value::as_str) == Some("assignment_action_unoffered")
            && content.get("next_action").and_then(Value::as_str) == Some("list_current_offers")
        {
            return Ok(ManagedTurnActionSubmissionV1::OfferUnavailable);
        }
        bail!("assignment MCP tool returned a closed failure")
    }

    fn fail_managed_turn(&mut self) -> Result<Value> {
        let result = self.request(
            "tools/call",
            &json!({
                "name": "worldstream.fail_managed_turn",
                "arguments": {},
            }),
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(false) {
            let content = result
                .get("structuredContent")
                .cloned()
                .context("assignment MCP response is invalid")?;
            if content.get("schema").and_then(Value::as_str)
                == Some("worldstream/managed-turn-failure/v1")
                && content.get("action_submitted").and_then(Value::as_bool) == Some(false)
            {
                return Ok(content);
            }
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
