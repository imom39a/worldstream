#![allow(clippy::panic, reason = "test fixture failures require diagnostics")]

use std::{
    fmt::Debug,
    sync::{Mutex, MutexGuard},
};

use worldstream_core::{
    CanonicalJsonV1, CanonicalPackOperationCodecV1, PackFaultV1, PackRevisionDescriptorV1,
    builtin_counter_registry,
};

use super::{
    CALLBACK_CONCURRENCY_LIMIT, CALLBACK_FUEL, CALLBACK_NATIVE_STACK_BYTES,
    COMPILE_CONCURRENCY_LIMIT, COMPONENT_MAX_INPUT_BYTES, COMPONENT_MAX_OUTPUT_BYTES,
    COMPONENT_MAX_WASM_STACK_BYTES, ComponentHostErrorV1, ComponentPackAdapterV1,
    ComponentPackHostV1, ConcurrencyGate, Export, FAULT_FUEL_EXHAUSTED, FAULT_HOST_UNAVAILABLE,
    FAULT_INPUT_ENCODING, FAULT_INPUT_TOO_LARGE, FAULT_OUTPUT_NOT_CANONICAL, FAULT_OUTPUT_SHAPE,
    FAULT_OUTPUT_TOO_LARGE, FAULT_RESOURCE_LIMIT, FAULT_TRAP, InvokeError, STORE_INSTANCE_LIMIT,
    STORE_LINEAR_MEMORY_BYTES, STORE_MEMORY_LIMIT, STORE_TABLE_ELEMENT_LIMIT, STORE_TABLE_LIMIT,
    canonical_bytes,
};

#[derive(Clone, Copy)]
enum Body {
    Return,
    Trap,
    FuelLoop,
    StackRecursion,
}

static COMPONENT_COMPILE_TEST_GATE: Mutex<()> = Mutex::new(());

fn serialize_component_compilation() -> MutexGuard<'static, ()> {
    COMPONENT_COMPILE_TEST_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn must<T, E: Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("test fixture must succeed: {error:?}"))
}

fn descriptor() -> PackRevisionDescriptorV1 {
    let registry = must(builtin_counter_registry());
    registry
        .catalog_revisions()
        .next()
        .unwrap_or_else(|| unreachable!("built-in Counter registry is nonempty"))
        .descriptor
}

fn escape_wat_data(bytes: &[u8]) -> String {
    let mut escaped = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut escaped, "\\{byte:02x}")
            .unwrap_or_else(|_| unreachable!("String writes are infallible"));
    }
    escaped
}

#[allow(clippy::too_many_lines)]
fn component(
    output: &[u8],
    advertised_length: usize,
    memory_pages: usize,
    body: Body,
    extra_export: bool,
    wrong_view_signature: bool,
) -> Vec<u8> {
    let output_data = escape_wat_data(output);
    let return_area = output.len().saturating_add(19) & !3;
    let heap_start = return_area.saturating_add(16);
    let body = match body {
        Body::Return => "call $write-result".to_owned(),
        Body::Trap => "unreachable".to_owned(),
        Body::FuelLoop => "loop $forever br $forever end unreachable".to_owned(),
        Body::StackRecursion => "call $recurse".to_owned(),
    };
    let extra_export = if extra_export {
        "(export \"extra\" (func $view-export))"
    } else {
        ""
    };
    let view_type = if wrong_view_signature {
        "$descriptor-type"
    } else {
        "$callback-type"
    };
    let view_core = if wrong_view_signature {
        "$descriptor"
    } else {
        "$view"
    };
    let wat = format!(
        r#"
(component
  (core module $module
    (memory (export "memory") {memory_pages})
    (data (i32.const 0) "{output_data}")
    (global $heap (mut i32) (i32.const {heap_start}))
    (global $called (mut i32) (i32.const 0))
    (func $realloc (export "cabi_realloc")
      (param $old i32) (param $old-size i32) (param $align i32) (param $new-size i32)
      (result i32)
      (local $pointer i32)
      global.get $heap
      local.tee $pointer
      local.get $new-size
      i32.add
      global.set $heap
      local.get $pointer)
    (func $write-result (result i32)
      i32.const {return_area}
      i32.const 0
      i32.store
      i32.const {return_area}
      i32.const {advertised_length}
      i32.store offset=4
      i32.const {return_area})
    (func $recurse (result i32) call $recurse)
    (func $guard
      global.get $called
      if unreachable end
      i32.const 1
      global.set $called)
    (func (export "descriptor") (result i32)
      call $guard
      {body})
    (func (export "initialize") (param i32 i32) (result i32)
      call $guard
      {body})
    (func (export "reduce") (param i32 i32) (result i32)
      call $guard
      {body})
    (func (export "view") (param i32 i32) (result i32)
      call $guard
      {body})
    (func (export "observe") (param i32 i32) (result i32)
      call $guard
      {body}))
  (core instance $instance (instantiate $module))
  (alias core export $instance "memory" (core memory $memory))
  (alias core export $instance "cabi_realloc" (core func $realloc))
  (alias core export $instance "descriptor" (core func $descriptor))
  (alias core export $instance "initialize" (core func $initialize))
  (alias core export $instance "reduce" (core func $reduce))
  (alias core export $instance "view" (core func $view))
  (alias core export $instance "observe" (core func $observe))
  (type $bytes (list u8))
  (type $descriptor-type (func (result $bytes)))
  (type $callback-type (func (param "input" $bytes) (result $bytes)))
  (func $descriptor-export (type $descriptor-type)
    (canon lift (core func $descriptor) (memory $memory)))
  (func $initialize-export (type $callback-type)
    (canon lift (core func $initialize) (memory $memory) (realloc $realloc)))
  (func $reduce-export (type $callback-type)
    (canon lift (core func $reduce) (memory $memory) (realloc $realloc)))
  (func $view-export (type {view_type})
    (canon lift (core func {view_core}) (memory $memory) (realloc $realloc)))
  (func $observe-export (type $callback-type)
    (canon lift (core func $observe) (memory $memory) (realloc $realloc)))
  (export "descriptor" (func $descriptor-export))
  (export "initialize" (func $initialize-export))
  (export "reduce" (func $reduce-export))
  (export "view" (func $view-export))
  (export "observe" (func $observe-export))
  {extra_export})"#,
    );
    must(wat::parse_str(wat))
}

fn adapter(bytes: &[u8]) -> Result<ComponentPackAdapterV1, ComponentHostErrorV1> {
    let host = ComponentPackHostV1::new()?;
    ComponentPackAdapterV1::compile(&host.engine, bytes, descriptor())
}

#[test]
fn malformed_imported_and_wrong_export_components_are_rejected() {
    let _test_guard = serialize_component_compilation();
    assert_eq!(
        adapter(b"not a WebAssembly Component").err(),
        Some(ComponentHostErrorV1::ComponentRejected)
    );

    let imported = must(wat::parse_str(
        r#"(component
            (type $run (func))
            (import "wasi:cli/run@0.2.0" (func (type $run))))"#,
    ));
    assert_eq!(
        adapter(&imported).err(),
        Some(ComponentHostErrorV1::ForbiddenImport)
    );

    let valid_output = canonical_bytes(&descriptor().content())
        .unwrap_or_else(|()| unreachable!("Counter descriptor is canonical"));
    let extra = component(
        &valid_output,
        valid_output.len(),
        2,
        Body::Return,
        true,
        false,
    );
    assert_eq!(
        adapter(&extra).err(),
        Some(ComponentHostErrorV1::ExportContractMismatch)
    );
    let wrong_signature = component(
        &valid_output,
        valid_output.len(),
        2,
        Body::Return,
        false,
        true,
    );
    assert_eq!(
        adapter(&wrong_signature).err(),
        Some(ComponentHostErrorV1::ExportContractMismatch)
    );
}

#[test]
fn descriptor_must_be_exact_canonical_content_without_revision_digest() {
    let _test_guard = serialize_component_compilation();
    let expected = canonical_bytes(&descriptor().content())
        .unwrap_or_else(|()| unreachable!("Counter descriptor content is canonical"));
    let exact = must(adapter(&component(
        &expected,
        expected.len(),
        2,
        Body::Return,
        false,
        false,
    )));
    assert_eq!(exact.verify_descriptor_content(), Ok(()));

    let full = canonical_bytes(&descriptor())
        .unwrap_or_else(|()| unreachable!("Counter descriptor is canonical"));
    let cyclic = must(adapter(&component(
        &full,
        full.len(),
        2,
        Body::Return,
        false,
        false,
    )));
    assert_eq!(
        cyclic.verify_descriptor_content(),
        Err(ComponentHostErrorV1::DescriptorContentMismatch)
    );

    let noncanonical = b"{ \"not\": \"canonical\" }";
    let malformed = must(adapter(&component(
        noncanonical,
        noncanonical.len(),
        2,
        Body::Return,
        false,
        false,
    )));
    assert_eq!(
        malformed.verify_descriptor_content(),
        Err(ComponentHostErrorV1::DescriptorContentMismatch)
    );
}

#[test]
fn every_callback_uses_a_fresh_store_and_instance() {
    let _test_guard = serialize_component_compilation();
    let response = br#"{"operation_result_type":"success","output":null}"#;
    let adapter = must(adapter(&component(
        response,
        response.len(),
        2,
        Body::Return,
        false,
        false,
    )));
    for _ in 0..3 {
        assert_eq!(must(adapter.invoke(Export::View, Some(b"{}"))), response);
    }
}

#[test]
fn independently_constructed_hosts_return_identical_callback_bytes() {
    let _test_guard = serialize_component_compilation();
    let response = br#"{"operation_result_type":"success","output":null}"#;
    let component = component(response, response.len(), 2, Body::Return, false, false);
    let first = must(adapter(&component));
    let second = must(adapter(&component));
    assert_eq!(
        first.invoke(Export::Observe, Some(b"{}")),
        second.invoke(Export::Observe, Some(b"{}"))
    );
}

#[test]
fn fuel_memory_trap_and_oversize_output_produce_no_bytes() {
    let _test_guard = serialize_component_compilation();
    let response = b"{}";
    for (body, fuel, expected) in [
        (Body::Trap, CALLBACK_FUEL, InvokeError::Trap),
        (Body::FuelLoop, 10_000, InvokeError::FuelExhausted),
    ] {
        let adapter = must(adapter(&component(
            response,
            response.len(),
            2,
            body,
            false,
            false,
        )));
        assert_eq!(
            adapter.invoke_with_fuel(Export::Reduce, Some(b"{}"), fuel),
            Err(expected)
        );
    }

    let oversized_memory_pages = STORE_LINEAR_MEMORY_BYTES / 65_536 + 1;
    let memory_hog = must(adapter(&component(
        response,
        response.len(),
        oversized_memory_pages,
        Body::Return,
        false,
        false,
    )));
    assert!(matches!(
        memory_hog.invoke(Export::Initialize, Some(b"{}")),
        Err(InvokeError::ResourceLimit)
    ));

    let advertised = COMPONENT_MAX_OUTPUT_BYTES + 1;
    let pages = advertised.div_ceil(65_536) + 1;
    let oversized_output = must(adapter(&component(
        &[],
        advertised,
        pages,
        Body::Return,
        false,
        false,
    )));
    assert!(matches!(
        oversized_output.invoke(Export::Observe, Some(b"{}")),
        Err(InvokeError::OutputTooLarge)
    ));
}

#[test]
fn recursive_component_exhausts_wasm_stack_without_aborting_the_host() {
    let _test_guard = serialize_component_compilation();
    let response = b"{}";
    let adapter = must(adapter(&component(
        response,
        response.len(),
        2,
        Body::StackRecursion,
        false,
        false,
    )));
    assert_eq!(
        adapter.invoke(Export::Reduce, Some(b"{}")),
        Err(InvokeError::ResourceLimit)
    );
}

#[test]
fn malformed_callback_data_is_canonical_fail_closed_and_redacted() {
    let _test_guard = serialize_component_compilation();
    let response = b"{ \"operation_result_type\": \"success\", \"output\": null }";
    let adapter = must(adapter(&component(
        response,
        response.len(),
        2,
        Body::Return,
        false,
        false,
    )));
    let bytes = must(adapter.invoke(Export::View, Some(b"{}")));
    assert!(
        CanonicalPackOperationCodecV1::canonical_v1()
            .decode_view_result(&bytes)
            .is_err()
    );
    assert_eq!(
        ComponentPackAdapterV1::output_decode_fault(
            &worldstream_core::CanonicalJsonError::NonCanonicalBytes,
        ),
        PackFaultV1::Callback(FAULT_OUTPUT_NOT_CANONICAL.to_owned())
    );
    assert!(CanonicalJsonV1::from_canonical_bytes(&bytes).is_err());
}

#[test]
fn input_limit_is_enforced_before_component_instantiation() {
    let _test_guard = serialize_component_compilation();
    let response = b"{}";
    let adapter = must(adapter(&component(
        response,
        response.len(),
        2,
        Body::Return,
        false,
        false,
    )));
    let oversized = vec![0; super::COMPONENT_MAX_INPUT_BYTES + 1];
    assert!(matches!(
        adapter.invoke(Export::View, Some(&oversized)),
        Err(InvokeError::InputTooLarge)
    ));
}

#[test]
fn accepted_profile_and_nonblocking_concurrency_gate_are_frozen() {
    assert_eq!(COMPILE_CONCURRENCY_LIMIT, 1);
    assert_eq!(CALLBACK_CONCURRENCY_LIMIT, 4);
    assert_eq!(CALLBACK_FUEL, 500_000_000);
    assert_eq!(COMPONENT_MAX_INPUT_BYTES, 1024 * 1024);
    assert_eq!(COMPONENT_MAX_OUTPUT_BYTES, 1024 * 1024);
    assert_eq!(STORE_LINEAR_MEMORY_BYTES, 128 * 1024 * 1024);
    assert_eq!(STORE_TABLE_ELEMENT_LIMIT, 100_000);
    assert_eq!(STORE_INSTANCE_LIMIT, 2_000);
    assert_eq!(STORE_TABLE_LIMIT, 2_000);
    assert_eq!(STORE_MEMORY_LIMIT, 256);
    assert_eq!(COMPONENT_MAX_WASM_STACK_BYTES, 2 * 1024 * 1024);
    assert_eq!(CALLBACK_NATIVE_STACK_BYTES, 8 * 1024 * 1024);
    const { assert!(CALLBACK_NATIVE_STACK_BYTES > COMPONENT_MAX_WASM_STACK_BYTES) };

    let gate = ConcurrencyGate::new(1);
    let permit = gate
        .try_acquire()
        .unwrap_or_else(|| unreachable!("first slot is available"));
    assert!(gate.try_acquire().is_none());
    drop(permit);
    assert!(gate.try_acquire().is_some());
}

#[test]
fn host_fault_classes_are_stable_and_redacted() {
    let cases = [
        (InvokeError::HostUnavailable, FAULT_HOST_UNAVAILABLE),
        (InvokeError::InputTooLarge, FAULT_INPUT_TOO_LARGE),
        (InvokeError::OutputTooLarge, FAULT_OUTPUT_TOO_LARGE),
        (InvokeError::FuelExhausted, FAULT_FUEL_EXHAUSTED),
        (InvokeError::ResourceLimit, FAULT_RESOURCE_LIMIT),
        (InvokeError::Trap, FAULT_TRAP),
        (InvokeError::OutputShape, FAULT_OUTPUT_SHAPE),
    ];
    for (error, expected) in cases {
        assert_eq!(
            ComponentPackAdapterV1::invoke_fault(error),
            PackFaultV1::Callback(expected.to_owned())
        );
    }
    assert_eq!(
        ComponentPackAdapterV1::input_encoding_fault(),
        PackFaultV1::Callback(FAULT_INPUT_ENCODING.to_owned())
    );
}
