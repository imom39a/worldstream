use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::Serialize;
use wasmtime::component::{
    Component, Linker,
    types::{ComponentItem, Type},
};
use wasmtime::{
    Config, Engine, Error as WasmtimeError, OutOfMemory, Store, StoreLimits, StoreLimitsBuilder,
    Trap,
};
use worldstream_core::{
    ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1, CanonicalJsonError,
    CanonicalJsonV1, CanonicalPackOperationCodecV1, DeterministicContextV1, InitialOutputV1,
    ObserveInputV1, PackFaultV1, PackObservationV1, PackRegistryStatusV1, PackRevisionDescriptorV1,
    PackViewV1, PortablePackAdmissionV1, ViewInputV1,
};
use worldstream_pack_bundle::{MAX_COMPONENT_BYTES, VerifiedPackBundleV1};

use crate::ComponentHostErrorV1;

const EXPORT_DESCRIPTOR: &str = "descriptor";
const EXPORT_INITIALIZE: &str = "initialize";
const EXPORT_REDUCE: &str = "reduce";
const EXPORT_VIEW: &str = "view";
const EXPORT_OBSERVE: &str = "observe";
const REQUIRED_EXPORTS: [&str; 5] = [
    EXPORT_DESCRIPTOR,
    EXPORT_INITIALIZE,
    EXPORT_REDUCE,
    EXPORT_VIEW,
    EXPORT_OBSERVE,
];

const FAULT_INPUT_ENCODING: &str = "portable_executor/input_encoding";
const FAULT_INPUT_TOO_LARGE: &str = "portable_executor/input_too_large";
const FAULT_FUEL_EXHAUSTED: &str = "portable_executor/fuel_exhausted";
const FAULT_RESOURCE_LIMIT: &str = "portable_executor/resource_limit";
const FAULT_TRAP: &str = "portable_executor/trap";
const FAULT_OUTPUT_TOO_LARGE: &str = "portable_executor/output_too_large";
const FAULT_OUTPUT_NOT_CANONICAL: &str = "portable_executor/output_not_canonical";
const FAULT_OUTPUT_SHAPE: &str = "portable_executor/output_shape";
const FAULT_HOST_UNAVAILABLE: &str = "portable_executor/host_unavailable";
const CALLBACK_NATIVE_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Maximum concurrent compilation attempts across this process.
pub const COMPILE_CONCURRENCY_LIMIT: usize = 1;
/// Maximum concurrent portable callbacks across this process.
pub const CALLBACK_CONCURRENCY_LIMIT: usize = 4;
/// Fuel installed in every fresh callback Store.
pub const CALLBACK_FUEL: u64 = 500_000_000;
/// Maximum canonical request bytes copied into a Component callback.
pub const COMPONENT_MAX_INPUT_BYTES: usize = 1024 * 1024;
/// Maximum callback bytes accepted before canonical decoding.
pub const COMPONENT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Maximum linear-memory size for each memory in a callback Store.
pub const STORE_LINEAR_MEMORY_BYTES: usize = 128 * 1024 * 1024;
/// Maximum elements in each table in a callback Store.
pub const STORE_TABLE_ELEMENT_LIMIT: usize = 100_000;
/// Maximum core instances created in one callback Store.
pub const STORE_INSTANCE_LIMIT: usize = 2_000;
/// Maximum linear memories created in one callback Store.
pub const STORE_MEMORY_LIMIT: usize = 256;
/// Maximum tables created in one callback Store.
pub const STORE_TABLE_LIMIT: usize = 2_000;
/// Maximum native stack reserved for WebAssembly execution.
pub const COMPONENT_MAX_WASM_STACK_BYTES: usize = 2 * 1024 * 1024;

static COMPILE_GATE: ConcurrencyGate = ConcurrencyGate::new(COMPILE_CONCURRENCY_LIMIT);
static CALLBACK_GATE: ConcurrencyGate = ConcurrencyGate::new(CALLBACK_CONCURRENCY_LIMIT);

/// Fixed deterministic host profile for approved portable Activity Packs.
#[derive(Clone)]
pub struct ComponentPackHostV1 {
    engine: Engine,
}

impl ComponentPackHostV1 {
    /// Creates the frozen synchronous Wasmtime execution profile.
    ///
    /// # Errors
    ///
    /// Returns a stable error if this process cannot initialize the pinned
    /// Wasmtime engine under the required deterministic configuration.
    pub fn new() -> Result<Self, ComponentHostErrorV1> {
        let mut config = Config::new();
        // The Wasmtime dependency deliberately omits every async/WASI feature;
        // there is no async Component surface to enable in this build.
        config
            .wasm_component_model(true)
            .wasm_threads(false)
            .wasm_memory64(false)
            .wasm_relaxed_simd(false)
            .relaxed_simd_deterministic(true)
            .consume_fuel(true)
            .max_wasm_stack(COMPONENT_MAX_WASM_STACK_BYTES)
            .cranelift_nan_canonicalization(true)
            .parallel_compilation(false);
        let engine = Engine::new(&config).map_err(|_| ComponentHostErrorV1::ComponentRejected)?;
        Ok(Self { engine })
    }

    /// Compiles and preflights one exact verified Component and converts it to
    /// the sole Core portable-admission value.
    ///
    /// The verified bundle is consumed. Operator inventory supplies status
    /// separately because selection and retained-runnable state are not facts
    /// carried by immutable bundle bytes.
    ///
    /// # Errors
    ///
    /// Returns a stable, redacted contract error. No Wasmtime diagnostic or
    /// partially constructed executor crosses this boundary.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "admission deliberately consumes the verified bundle authority"
    )]
    pub fn admit(
        &self,
        bundle: VerifiedPackBundleV1,
        status: PackRegistryStatusV1,
    ) -> Result<PortablePackAdmissionV1, ComponentHostErrorV1> {
        let adapter = Arc::new(ComponentPackAdapterV1::compile(
            &self.engine,
            bundle.component_bytes(),
            bundle.descriptor().clone(),
        )?);
        adapter.verify_descriptor_content()?;

        Ok(PortablePackAdmissionV1::new(
            bundle.revision_lock().clone(),
            bundle.descriptor().clone(),
            bundle.schemas().clone(),
            bundle.codecs().clone(),
            bundle.component_digest().clone(),
            bundle.golden_corpus_digest().clone(),
            bundle.golden_corpus().clone(),
            adapter,
            status,
        ))
    }
}

struct ComponentPackAdapterV1 {
    engine: Engine,
    component: Component,
    descriptor: PackRevisionDescriptorV1,
}

impl ComponentPackAdapterV1 {
    fn compile(
        engine: &Engine,
        original_component: &[u8],
        descriptor: PackRevisionDescriptorV1,
    ) -> Result<Self, ComponentHostErrorV1> {
        if original_component.is_empty() || original_component.len() > MAX_COMPONENT_BYTES {
            return Err(ComponentHostErrorV1::ComponentRejected);
        }
        let _permit = COMPILE_GATE
            .try_acquire()
            .ok_or(ComponentHostErrorV1::CompileConcurrencyLimit)?;
        // Deliberately compile the verified original bytes. There is no AOT
        // deserialization or cache-authority path in this host.
        let component = Component::from_binary(engine, original_component)
            .map_err(|_| ComponentHostErrorV1::ComponentRejected)?;
        validate_component_contract(engine, &component)?;
        Ok(Self {
            engine: engine.clone(),
            component,
            descriptor,
        })
    }

    fn verify_descriptor_content(&self) -> Result<(), ComponentHostErrorV1> {
        let actual = self
            .invoke(Export::Descriptor, None)
            .map_err(|_| ComponentHostErrorV1::DescriptorCallbackFailed)?;
        let expected_content = self.descriptor.content();
        let expected = canonical_bytes(&expected_content)
            .map_err(|()| ComponentHostErrorV1::DescriptorContentMismatch)?;
        let decoded = CanonicalPackOperationCodecV1::canonical_v1()
            .decode_descriptor(&actual)
            .map_err(|_| ComponentHostErrorV1::DescriptorContentMismatch)?;
        if actual != expected || decoded != expected_content {
            return Err(ComponentHostErrorV1::DescriptorContentMismatch);
        }
        Ok(())
    }

    fn invoke(&self, export: Export, input: Option<&[u8]>) -> Result<Vec<u8>, InvokeError> {
        self.invoke_with_fuel(export, input, CALLBACK_FUEL)
    }

    fn invoke_with_fuel(
        &self,
        export: Export,
        input: Option<&[u8]>,
        fuel: u64,
    ) -> Result<Vec<u8>, InvokeError> {
        if input.is_some_and(|bytes| bytes.len() > COMPONENT_MAX_INPUT_BYTES) {
            return Err(InvokeError::InputTooLarge);
        }
        let _permit = CALLBACK_GATE
            .try_acquire()
            .ok_or(InvokeError::HostUnavailable)?;
        let engine = self.engine.clone();
        let component = self.component.clone();
        let input = input.map(<[u8]>::to_vec);
        let callback = std::thread::Builder::new()
            .name("worldstream-pack-callback".to_owned())
            // Wasmtime requires max_wasm_stack to remain below the native
            // caller stack. A joined dedicated thread makes the frozen 2 MiB
            // WebAssembly limit safe even when the server caller uses a small
            // runtime-worker stack.
            .stack_size(CALLBACK_NATIVE_STACK_BYTES)
            .spawn(move || {
                Self::invoke_on_fresh_instance(&engine, &component, export, input.as_deref(), fuel)
            })
            .map_err(|_| InvokeError::HostUnavailable)?;
        callback.join().map_err(|_| InvokeError::Trap)?
    }

    fn invoke_on_fresh_instance(
        engine: &Engine,
        component: &Component,
        export: Export,
        input: Option<&[u8]>,
        fuel: u64,
    ) -> Result<Vec<u8>, InvokeError> {
        let mut store = fresh_store(engine, fuel)?;
        let linker = Linker::<StoreData>::new(engine);
        let instance = linker
            .instantiate(&mut store, component)
            .map_err(|error| classify_instantiation_error(&error))?;
        let output = match input {
            None => {
                let function = instance
                    .get_typed_func::<(), (Vec<u8>,)>(&mut store, export.name())
                    .map_err(|_| InvokeError::OutputShape)?;
                // Wasmtime 48's typed synchronous `call` copies the lifted
                // result and automatically runs canonical ABI post-return
                // before it returns. Dropping this fresh Store therefore
                // happens only after the complete one-call lifecycle.
                let (output,) = function
                    .call(&mut store, ())
                    .map_err(|error| classify_call_error(&error))?;
                output
            }
            Some(input) => {
                let function = instance
                    .get_typed_func::<(Vec<u8>,), (Vec<u8>,)>(&mut store, export.name())
                    .map_err(|_| InvokeError::OutputShape)?;
                let (output,) = function
                    .call(&mut store, (input.to_vec(),))
                    .map_err(|error| classify_call_error(&error))?;
                output
            }
        };
        if output.len() > COMPONENT_MAX_OUTPUT_BYTES {
            return Err(InvokeError::OutputTooLarge);
        }
        Ok(output)
    }

    fn invoke_fault(error: InvokeError) -> PackFaultV1 {
        let detail = match error {
            InvokeError::HostUnavailable => FAULT_HOST_UNAVAILABLE,
            InvokeError::InputTooLarge => FAULT_INPUT_TOO_LARGE,
            InvokeError::OutputTooLarge => FAULT_OUTPUT_TOO_LARGE,
            InvokeError::FuelExhausted => FAULT_FUEL_EXHAUSTED,
            InvokeError::ResourceLimit => FAULT_RESOURCE_LIMIT,
            InvokeError::Trap => FAULT_TRAP,
            InvokeError::OutputShape => FAULT_OUTPUT_SHAPE,
        };
        PackFaultV1::Callback(detail.to_owned())
    }

    fn input_encoding_fault() -> PackFaultV1 {
        PackFaultV1::Callback(FAULT_INPUT_ENCODING.to_owned())
    }

    fn output_decode_fault(error: &CanonicalJsonError) -> PackFaultV1 {
        let detail = if matches!(error, CanonicalJsonError::TypedDecode(_)) {
            FAULT_OUTPUT_SHAPE
        } else {
            FAULT_OUTPUT_NOT_CANONICAL
        };
        PackFaultV1::Callback(detail.to_owned())
    }
}

impl ActivityPackV1 for ComponentPackAdapterV1 {
    fn descriptor(&self) -> &PackRevisionDescriptorV1 {
        &self.descriptor
    }

    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        context: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> {
        let codec = CanonicalPackOperationCodecV1::canonical_v1();
        let request = codec
            .encode_initialize_request(input, context)
            .map_err(|_| Self::input_encoding_fault())?;
        let response = self
            .invoke(Export::Initialize, Some(&request))
            .map_err(Self::invoke_fault)?;
        codec
            .decode_initialize_result(&response)
            .map_err(|error| Self::output_decode_fault(&error))?
            .into_result()
    }

    fn reduce(
        &self,
        input: &worldstream_core::ActivityReduceInputV1<'_>,
        context: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> {
        let codec = CanonicalPackOperationCodecV1::canonical_v1();
        let request = codec
            .encode_reduce_request(input, context)
            .map_err(|_| Self::input_encoding_fault())?;
        let response = self
            .invoke(Export::Reduce, Some(&request))
            .map_err(Self::invoke_fault)?;
        codec
            .decode_reduce_result(&response)
            .map_err(|error| Self::output_decode_fault(&error))?
            .into_result()
    }

    fn view(&self, input: &ViewInputV1<'_>) -> Result<PackViewV1, PackFaultV1> {
        let codec = CanonicalPackOperationCodecV1::canonical_v1();
        let request = codec
            .encode_view_request(input)
            .map_err(|_| Self::input_encoding_fault())?;
        let response = self
            .invoke(Export::View, Some(&request))
            .map_err(Self::invoke_fault)?;
        codec
            .decode_view_result(&response)
            .map_err(|error| Self::output_decode_fault(&error))?
            .into_result()
    }

    fn observe(
        &self,
        input: &ObserveInputV1<'_>,
    ) -> Result<Option<PackObservationV1>, PackFaultV1> {
        let codec = CanonicalPackOperationCodecV1::canonical_v1();
        let request = codec
            .encode_observe_request(input)
            .map_err(|_| Self::input_encoding_fault())?;
        let response = self
            .invoke(Export::Observe, Some(&request))
            .map_err(Self::invoke_fault)?;
        codec
            .decode_observe_result(&response, input.after_view)
            .map_err(|error| Self::output_decode_fault(&error))?
            .into_result()
    }
}

fn validate_component_contract(
    engine: &Engine,
    component: &Component,
) -> Result<(), ComponentHostErrorV1> {
    let component_type = component.component_type();
    if component_type.imports(engine).len() != 0 {
        return Err(ComponentHostErrorV1::ForbiddenImport);
    }
    let exports: Vec<_> = component_type.exports(engine).collect();
    let names: BTreeSet<_> = exports.iter().map(|(name, _)| *name).collect();
    let expected: BTreeSet<_> = REQUIRED_EXPORTS.into_iter().collect();
    if exports.len() != REQUIRED_EXPORTS.len() || names != expected {
        return Err(ComponentHostErrorV1::ExportContractMismatch);
    }
    for (name, export) in exports {
        let ComponentItem::ComponentFunc(function) = export.ty else {
            return Err(ComponentHostErrorV1::ExportContractMismatch);
        };
        let parameter_count = usize::from(name != EXPORT_DESCRIPTOR);
        if function.params().len() != parameter_count
            || !function.params().all(|(_, ty)| is_byte_list(&ty))
            || function.results().len() != 1
            || !function.results().all(|ty| is_byte_list(&ty))
        {
            return Err(ComponentHostErrorV1::ExportContractMismatch);
        }
    }
    Ok(())
}

fn is_byte_list(ty: &Type) -> bool {
    matches!(ty, Type::List(list) if list.ty() == Type::U8)
}

fn fresh_store(engine: &Engine, fuel: u64) -> Result<Store<StoreData>, InvokeError> {
    let limits = StoreLimitsBuilder::new()
        .memory_size(STORE_LINEAR_MEMORY_BYTES)
        .table_elements(STORE_TABLE_ELEMENT_LIMIT)
        .instances(STORE_INSTANCE_LIMIT)
        .memories(STORE_MEMORY_LIMIT)
        .tables(STORE_TABLE_LIMIT)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(engine, StoreData { limits });
    store.limiter(|data| &mut data.limits);
    store
        .set_fuel(fuel)
        .map_err(|_| InvokeError::HostUnavailable)?;
    Ok(store)
}

fn classify_instantiation_error(error: &WasmtimeError) -> InvokeError {
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => InvokeError::FuelExhausted,
        Some(Trap::StackOverflow | Trap::AllocationTooLarge) | None => InvokeError::ResourceLimit,
        Some(_) => InvokeError::Trap,
    }
}

fn classify_call_error(error: &WasmtimeError) -> InvokeError {
    if error.downcast_ref::<OutOfMemory>().is_some() {
        return InvokeError::ResourceLimit;
    }
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => InvokeError::FuelExhausted,
        Some(Trap::StackOverflow | Trap::AllocationTooLarge) => InvokeError::ResourceLimit,
        Some(_) => InvokeError::Trap,
        // Canonical ABI lowering/lifting and automatic post-return failures
        // are output-shape failures; their diagnostics stay host-private.
        None => InvokeError::OutputShape,
    }
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ()> {
    let serialized = serde_json::to_vec(value).map_err(|_| ())?;
    CanonicalJsonV1::parse(&serialized)
        .and_then(|canonical| canonical.to_bytes())
        .map_err(|_| ())
}

struct StoreData {
    limits: StoreLimits,
}

#[derive(Clone, Copy)]
enum Export {
    Descriptor,
    Initialize,
    Reduce,
    View,
    Observe,
}

impl Export {
    const fn name(self) -> &'static str {
        match self {
            Self::Descriptor => EXPORT_DESCRIPTOR,
            Self::Initialize => EXPORT_INITIALIZE,
            Self::Reduce => EXPORT_REDUCE,
            Self::View => EXPORT_VIEW,
            Self::Observe => EXPORT_OBSERVE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InvokeError {
    HostUnavailable,
    InputTooLarge,
    OutputTooLarge,
    FuelExhausted,
    ResourceLimit,
    Trap,
    OutputShape,
}

struct ConcurrencyGate {
    active: AtomicUsize,
    limit: usize,
}

impl ConcurrencyGate {
    const fn new(limit: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            limit,
        }
    }

    fn try_acquire(&self) -> Option<ConcurrencyPermit<'_>> {
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(ConcurrencyPermit { gate: self }),
                Err(actual) => current = actual,
            }
        }
    }
}

struct ConcurrencyPermit<'a> {
    gate: &'a ConcurrencyGate,
}

impl Drop for ConcurrencyPermit<'_> {
    fn drop(&mut self) {
        self.gate.active.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
