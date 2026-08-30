use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

const NORMAL_FUEL: u64 = 500_000_000;
const PROBE_FUEL: u64 = 25_000_000;
const MEMORY_LIMIT: usize = 128 * 1024 * 1024;
const BOUNDARY_LIMIT: usize = 64 * 1024;
const EXPECTED_EXPORTS: [&str; 5] = ["descriptor", "initialize", "observe", "reduce", "view"];

struct HostState {
    limits: StoreLimits,
}

struct Invocation {
    output: Vec<u8>,
    elapsed: Duration,
    fuel_remaining: u64,
}

struct ScenarioRun {
    transcript: Vec<Vec<u8>>,
    compile_elapsed: Duration,
    callback_elapsed: Duration,
}

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let source_component = PathBuf::from(
        args.next()
            .context("usage: prototype-host <component.wasm> <prototype-pack-store>")?,
    );
    let store_root = PathBuf::from(
        args.next()
            .context("usage: prototype-host <component.wasm> <prototype-pack-store>")?,
    );
    ensure!(args.next().is_none(), "unexpected extra arguments");

    let original = fs::read(&source_component)
        .with_context(|| format!("read {}", source_component.display()))?;
    let digest = sha256_hex(&original);
    let installed = stage_bundle(&store_root, &digest, &original)?;

    let first = run_after_startup(&installed)?;
    let second = run_after_startup(&installed)?;
    ensure!(
        first.transcript == second.transcript,
        "restart Replay changed canonical callback bytes"
    );

    let engine = deterministic_engine()?;
    let component = Component::new(&engine, &original)
        .map_err(anyhow::Error::from)
        .context("compile retained component")?;
    validate_component_shape(&engine, &component)?;
    let cache = component
        .serialize()
        .map_err(anyhow::Error::from)
        .context("serialize disposable engine cache")?;
    let cache_path = installed
        .parent()
        .context("installed component has no bundle directory")?
        .join("wasmtime-48.prototype-cache");
    fs::write(&cache_path, &cache).context("write disposable prototype cache")?;

    prove_forbidden_import_policy(&engine)?;
    let resource_evidence = prove_resource_containment(&engine, &component)?;

    let transcript_digest = sha256_hex(&first.transcript.concat());
    let evidence = json!({
        "boundary_limit_bytes": BOUNDARY_LIMIT,
        "cache": {
            "authoritative": false,
            "bytes": cache.len(),
            "sha256": sha256_hex(&cache)
        },
        "component": {
            "bytes": original.len(),
            "imports": [],
            "original_bytes_retained": true,
            "sha256": digest
        },
        "execution_profile": {
            "component_async": false,
            "component_threading": false,
            "fuel_per_callback": NORMAL_FUEL,
            "hostcall_fuel_bytes": BOUNDARY_LIMIT,
            "memory_limit_bytes": MEMORY_LIMIT,
            "nan_canonicalization": true,
            "relaxed_simd": false,
            "trap_on_grow_failure": true,
            "wasm_threads": false
        },
        "proofs": {
            "canonical_native_oracle_parity": true,
            "forbidden_import_rejected": true,
            "fresh_instance_per_callback": true,
            "resource_containment": resource_evidence,
            "restart_replay_equal": true,
            "startup_load_from_original_bytes": true
        },
        "timings_ms": {
            "first_compile": first.compile_elapsed.as_millis(),
            "first_callbacks": first.callback_elapsed.as_millis(),
            "restart_compile": second.compile_elapsed.as_millis(),
            "restart_callbacks": second.callback_elapsed.as_millis()
        },
        "transcript": {
            "callback_count": first.transcript.len(),
            "sha256": transcript_digest
        }
    });
    let evidence_bytes = canonical_value_bytes(&evidence)?;
    let evidence_path = store_root
        .parent()
        .unwrap_or(Path::new("."))
        .join("evidence.json");
    fs::write(&evidence_path, &evidence_bytes).context("write evidence.json")?;

    println!("PASS portable Activity Pack prototype");
    println!("component sha256: {}", evidence["component"]["sha256"]);
    println!("component bytes: {}", original.len());
    println!("transcript sha256: {}", evidence["transcript"]["sha256"]);
    println!("evidence: {}", evidence_path.display());
    Ok(())
}

fn deterministic_engine() -> Result<Engine> {
    let mut config = Config::new();
    config
        .wasm_component_model(true)
        .wasm_relaxed_simd(false)
        .relaxed_simd_deterministic(true)
        .cranelift_nan_canonicalization(true)
        .consume_fuel(true)
        .max_wasm_stack(2 * 1024 * 1024);
    Engine::new(&config)
        .map_err(anyhow::Error::from)
        .context("create deterministic Wasmtime engine")
}

fn new_store(engine: &Engine, fuel: u64) -> Result<Store<HostState>> {
    let limits = StoreLimitsBuilder::new()
        .memory_size(MEMORY_LIMIT)
        .table_elements(100_000)
        .instances(2_000)
        .tables(2_000)
        .memories(256)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(engine, HostState { limits });
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(fuel)
        .map_err(anyhow::Error::from)
        .context("set deterministic fuel")?;
    store.set_hostcall_fuel(BOUNDARY_LIMIT);
    Ok(store)
}

fn validate_component_shape(engine: &Engine, component: &Component) -> Result<()> {
    let imports = component
        .component_type()
        .imports(engine)
        .map(|(name, _)| name.to_owned())
        .collect::<Vec<_>>();
    ensure!(
        imports.is_empty(),
        "forbidden component imports: {imports:?}"
    );

    let exports = component
        .component_type()
        .exports(engine)
        .map(|(name, _)| name.to_owned())
        .collect::<BTreeSet<_>>();
    let expected = EXPECTED_EXPORTS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    ensure!(
        exports == expected,
        "unexpected component exports: {exports:?}"
    );
    Ok(())
}

fn invoke(
    engine: &Engine,
    component: &Component,
    operation: &str,
    input: &[u8],
    fuel: u64,
) -> Result<Invocation> {
    ensure!(
        input.len() <= BOUNDARY_LIMIT,
        "input exceeds deterministic boundary limit"
    );
    let mut store = new_store(engine, fuel)?;
    let linker = Linker::<HostState>::new(engine);
    let started = Instant::now();
    let instance = linker
        .instantiate(&mut store, component)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("instantiate for {operation}"))?;
    let output = if operation == "descriptor" {
        let function = instance
            .get_typed_func::<(), (Vec<u8>,)>(&mut store, operation)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("bind {operation}"))?;
        function
            .call(&mut store, ())
            .map_err(anyhow::Error::from)
            .with_context(|| format!("call {operation}"))?
            .0
    } else {
        let function = instance
            .get_typed_func::<(Vec<u8>,), (Vec<u8>,)>(&mut store, operation)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("bind {operation}"))?;
        function
            .call(&mut store, (input.to_vec(),))
            .map_err(anyhow::Error::from)
            .with_context(|| format!("call {operation}"))?
            .0
    };
    ensure!(
        output.len() <= BOUNDARY_LIMIT,
        "output exceeds deterministic boundary limit"
    );
    ensure!(
        canonical_json_bytes(&output)? == output,
        "{operation} returned non-canonical JSON"
    );
    let value: Value = serde_json::from_slice(&output).context("decode callback output")?;
    ensure!(
        value.get("instanceOrdinal") == Some(&Value::from(1)),
        "{operation} reused mutable guest instance state"
    );
    Ok(Invocation {
        output,
        elapsed: started.elapsed(),
        fuel_remaining: store
            .get_fuel()
            .map_err(anyhow::Error::from)
            .context("read remaining fuel")?,
    })
}

fn assert_call(
    engine: &Engine,
    component: &Component,
    operation: &str,
    input: &Value,
    expected: &Value,
    transcript: &mut Vec<Vec<u8>>,
) -> Result<Duration> {
    let input = canonical_value_bytes(input)?;
    let invocation = invoke(engine, component, operation, &input, NORMAL_FUEL)?;
    let expected = canonical_value_bytes(expected)?;
    ensure!(
        invocation.output == expected,
        "{operation} disagreed with native oracle\nactual: {}\nexpected: {}",
        String::from_utf8_lossy(&invocation.output),
        String::from_utf8_lossy(&expected)
    );
    ensure!(
        invocation.fuel_remaining < NORMAL_FUEL,
        "{operation} consumed no fuel"
    );
    transcript.push(invocation.output);
    Ok(invocation.elapsed)
}

fn run_after_startup(installed_component: &Path) -> Result<ScenarioRun> {
    let original = fs::read(installed_component).context("reload original component bytes")?;
    let engine = deterministic_engine()?;
    let compile_started = Instant::now();
    let component = Component::new(&engine, &original)
        .map_err(anyhow::Error::from)
        .context("compile after startup")?;
    let compile_elapsed = compile_started.elapsed();
    validate_component_shape(&engine, &component)?;

    let mut transcript = Vec::new();
    let mut callback_elapsed = Duration::ZERO;
    let empty = json!({});

    callback_elapsed += assert_call(
        &engine,
        &component,
        "descriptor",
        &empty,
        &json!({
            "actions": ["propose", "counter", "accept"],
            "instanceOrdinal": 1,
            "operations": ["descriptor", "initialize", "reduce", "view", "observe"],
            "packId": "worldstream.prototype.negotiation",
            "roles": ["buyer", "seller"],
            "semanticVersion": "0.1.0-prototype"
        }),
        &mut transcript,
    )?;

    let initial_state = state("negotiating", 0, Value::Null, Value::Null);
    callback_elapsed += assert_call(
        &engine,
        &component,
        "initialize",
        &json!({"configuration": {"buyerCeiling": 127, "sellerFloor": 93}}),
        &json!({
            "events": [{"type": "negotiation-opened"}],
            "instanceOrdinal": 1,
            "state": initial_state
        }),
        &mut transcript,
    )?;

    let buyer_proposal = json!({"price": 80, "proposedBy": "buyer", "revision": 1});
    let proposed_state = state("negotiating", 1, buyer_proposal.clone(), Value::Null);
    callback_elapsed += assert_call(
        &engine,
        &component,
        "reduce",
        &json!({
            "action": {"basisRevision": 0, "price": 80, "role": "buyer", "type": "propose"},
            "state": initial_state
        }),
        &json!({
            "events": [{"price": 80, "proposedBy": "buyer", "revision": 1, "type": "proposal-revised"}],
            "instanceOrdinal": 1,
            "kind": "applied",
            "state": proposed_state
        }),
        &mut transcript,
    )?;

    let seller_counter = json!({"price": 110, "proposedBy": "seller", "revision": 2});
    let countered_state = state("negotiating", 2, seller_counter.clone(), Value::Null);
    callback_elapsed += assert_call(
        &engine,
        &component,
        "reduce",
        &json!({
            "action": {"basisRevision": 1, "price": 110, "role": "seller", "type": "counter"},
            "state": proposed_state
        }),
        &json!({
            "events": [{"price": 110, "proposedBy": "seller", "revision": 2, "type": "proposal-revised"}],
            "instanceOrdinal": 1,
            "kind": "applied",
            "state": countered_state
        }),
        &mut transcript,
    )?;

    callback_elapsed += assert_call(
        &engine,
        &component,
        "view",
        &json!({"state": countered_state, "viewerRole": "buyer"}),
        &json!({
            "actionOffers": ["propose", "accept"],
            "instanceOrdinal": 1,
            "projection": {
                "currentProposal": seller_counter,
                "outcome": null,
                "phase": "negotiating",
                "privateLimit": 127,
                "revision": 2
            }
        }),
        &mut transcript,
    )?;

    callback_elapsed += assert_call(
        &engine,
        &component,
        "view",
        &json!({"state": countered_state, "viewerRole": "seller"}),
        &json!({
            "actionOffers": ["counter"],
            "instanceOrdinal": 1,
            "projection": {
                "currentProposal": seller_counter,
                "outcome": null,
                "phase": "negotiating",
                "privateLimit": 93,
                "revision": 2
            }
        }),
        &mut transcript,
    )?;

    callback_elapsed += assert_call(
        &engine,
        &component,
        "reduce",
        &json!({
            "action": {"basisRevision": 1, "role": "buyer", "type": "accept"},
            "state": countered_state
        }),
        &json!({
            "code": "stale-revision",
            "instanceOrdinal": 1,
            "kind": "rejected",
            "message": "Acceptance must name the exact current proposal.",
            "state": countered_state
        }),
        &mut transcript,
    )?;

    let outcome = json!({"acceptedBy": "buyer", "agreedPrice": 110});
    let agreed_state = state("agreed", 2, seller_counter.clone(), outcome.clone());
    callback_elapsed += assert_call(
        &engine,
        &component,
        "reduce",
        &json!({
            "action": {"basisRevision": 2, "role": "buyer", "type": "accept"},
            "state": countered_state
        }),
        &json!({
            "events": [{"agreedPrice": 110, "revision": 2, "type": "agreement-reached"}],
            "instanceOrdinal": 1,
            "kind": "applied",
            "state": agreed_state
        }),
        &mut transcript,
    )?;

    callback_elapsed += assert_call(
        &engine,
        &component,
        "observe",
        &json!({"after": agreed_state, "before": countered_state, "viewerRole": "buyer"}),
        &json!({
            "instanceOrdinal": 1,
            "summary": {
                "currentProposal": seller_counter,
                "outcome": outcome,
                "phase": "agreed",
                "revision": 2
            },
            "visible": true
        }),
        &mut transcript,
    )?;

    Ok(ScenarioRun {
        transcript,
        compile_elapsed,
        callback_elapsed,
    })
}

fn state(phase: &str, revision: u64, current_proposal: Value, outcome: Value) -> Value {
    json!({
        "buyerCeiling": 127,
        "currentProposal": current_proposal,
        "outcome": outcome,
        "phase": phase,
        "revision": revision,
        "sellerFloor": 93
    })
}

fn prove_forbidden_import_policy(engine: &Engine) -> Result<()> {
    let forbidden = Component::new(
        engine,
        r#"
            (component
                (type $clock (func))
                (import "wasi:clocks/monotonic-clock@0.2.0" (func (type $clock)))
            )
        "#,
    )
    .map_err(anyhow::Error::from)
    .context("compile forbidden-import fixture")?;
    ensure!(
        validate_component_shape(engine, &forbidden).is_err(),
        "forbidden-import component unexpectedly passed installation policy"
    );
    Ok(())
}

fn prove_resource_containment(engine: &Engine, component: &Component) -> Result<Value> {
    let state = state("negotiating", 0, Value::Null, Value::Null);
    for (name, action_type) in [
        ("fuel", "__probe_burn"),
        ("memory", "__probe_allocate"),
        ("stack", "__probe_recurse"),
        ("host_boundary", "__probe_oversize_output"),
    ] {
        let input = canonical_value_bytes(&json!({
            "action": {"role": "buyer", "type": action_type},
            "state": state
        }))?;
        let result = invoke(engine, component, "reduce", &input, PROBE_FUEL);
        ensure!(
            result.is_err(),
            "{name} probe unexpectedly returned a committable output"
        );
    }

    let oversized_input = vec![b'x'; BOUNDARY_LIMIT + 1];
    ensure!(
        invoke(engine, component, "reduce", &oversized_input, PROBE_FUEL).is_err(),
        "oversized input crossed the host boundary"
    );

    Ok(json!({
        "fuel_exhaustion_no_output": true,
        "growth_failure_traps": true,
        "host_boundary_excess_no_output": true,
        "oversized_input_rejected_before_guest": true,
        "stack_exhaustion_no_output": true
    }))
}

fn stage_bundle(store_root: &Path, digest: &str, original: &[u8]) -> Result<PathBuf> {
    let bundle_dir = store_root.join(digest);
    fs::create_dir_all(&bundle_dir).context("create prototype content-addressed bundle store")?;
    let component_path = bundle_dir.join("executor.component.wasm");
    if component_path.exists() {
        let retained = fs::read(&component_path).context("read retained component")?;
        ensure!(
            retained == original,
            "content-addressed bundle path contains different bytes"
        );
    } else {
        fs::write(&component_path, original).context("stage original component bytes")?;
    }
    let manifest = json!({
        "artifact": "executor.component.wasm",
        "component_sha256": digest,
        "execution_profile": "worldstream-prototype-component-v1",
        "host_contract": "worldstream/activity-pack/v1",
        "provisional": true
    });
    fs::write(
        bundle_dir.join("manifest.json"),
        canonical_value_bytes(&manifest)?,
    )
    .context("write prototype bundle manifest")?;
    Ok(component_path)
}

fn canonical_json_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let value: Value = serde_json::from_slice(bytes).context("parse JSON")?;
    canonical_value_bytes(&value)
}

fn canonical_value_bytes(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("encode canonical prototype JSON")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
