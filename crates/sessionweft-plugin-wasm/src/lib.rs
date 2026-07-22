use std::{path::Path, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder};

const ABI_MEMORY: &str = "memory";
const ABI_ALLOC: &str = "sessionweft_alloc";
const ABI_INVOKE: &str = "sessionweft_invoke_v1";
const ABI_DEALLOC: &str = "sessionweft_dealloc";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmPluginManifest {
    pub plugin_id: String,
    pub version: String,
    pub sha256: String,
    pub maximum_memory_bytes: usize,
    pub maximum_input_bytes: usize,
    pub maximum_output_bytes: usize,
    pub fuel: u64,
    pub timeout_millis: u64,
}

impl WasmPluginManifest {
    pub fn validate(&self) -> Result<(), WasmPluginError> {
        validate_identifier("plugin ID", &self.plugin_id, 128)?;
        validate_identifier("plugin version", &self.version, 64)?;
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(WasmPluginError::Manifest(
                "plugin SHA-256 must contain exactly 64 hexadecimal characters".into(),
            ));
        }
        if self.maximum_memory_bytes < 65_536 || self.maximum_memory_bytes > 1024 * 1024 * 1024 {
            return Err(WasmPluginError::Manifest(
                "maximum memory must be between 64 KiB and 1 GiB".into(),
            ));
        }
        if self.maximum_input_bytes == 0
            || self.maximum_output_bytes == 0
            || self.maximum_input_bytes > self.maximum_memory_bytes
            || self.maximum_output_bytes > self.maximum_memory_bytes
        {
            return Err(WasmPluginError::Manifest(
                "input and output limits must be non-zero and fit within memory".into(),
            ));
        }
        if self.fuel == 0 {
            return Err(WasmPluginError::Manifest(
                "plugin fuel must be greater than zero".into(),
            ));
        }
        if self.timeout_millis == 0 || self.timeout_millis > 300_000 {
            return Err(WasmPluginError::Manifest(
                "plugin timeout must be between 1 ms and 5 minutes".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmPluginResult {
    pub output: Vec<u8>,
    pub fuel_consumed: u64,
}

#[derive(Clone)]
pub struct PortableWasmSandbox {
    engine: Engine,
}

impl PortableWasmSandbox {
    pub fn new() -> Result<Self, WasmPluginError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        config.wasm_multi_memory(false);
        config.wasm_memory64(false);
        config.wasm_threads(false);
        config.wasm_reference_types(false);
        config.max_wasm_stack(512 * 1024);
        let engine = Engine::new(&config).map_err(engine_error)?;
        Ok(Self { engine })
    }

    pub async fn invoke_file(
        &self,
        manifest: WasmPluginManifest,
        module_path: impl AsRef<Path>,
        input: Vec<u8>,
    ) -> Result<WasmPluginResult, WasmPluginError> {
        let bytes = tokio::fs::read(module_path)
            .await
            .map_err(WasmPluginError::Io)?;
        self.invoke_bytes(manifest, bytes, input).await
    }

    pub async fn invoke_bytes(
        &self,
        manifest: WasmPluginManifest,
        module_bytes: Vec<u8>,
        input: Vec<u8>,
    ) -> Result<WasmPluginResult, WasmPluginError> {
        manifest.validate()?;
        if input.len() > manifest.maximum_input_bytes {
            return Err(WasmPluginError::InputTooLarge {
                actual: input.len(),
                limit: manifest.maximum_input_bytes,
            });
        }
        let actual_digest = hex_digest(&module_bytes);
        if !actual_digest.eq_ignore_ascii_case(&manifest.sha256) {
            return Err(WasmPluginError::IntegrityMismatch {
                expected: manifest.sha256,
                actual: actual_digest,
            });
        }
        let engine = self.engine.clone();
        let timeout = Duration::from_millis(manifest.timeout_millis);
        let epoch_engine = engine.clone();
        let interrupter = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            epoch_engine.increment_epoch();
        });
        let result = tokio::task::spawn_blocking(move || {
            invoke_blocking(&engine, &manifest, &module_bytes, &input)
        })
        .await
        .map_err(|error| WasmPluginError::Runtime(format!("sandbox worker failed: {error}")))?;
        interrupter.abort();
        result
    }
}

impl Default for PortableWasmSandbox {
    fn default() -> Self {
        Self::new().expect("portable Wasm sandbox configuration is valid")
    }
}

struct StoreState {
    limits: StoreLimits,
}

fn invoke_blocking(
    engine: &Engine,
    manifest: &WasmPluginManifest,
    module_bytes: &[u8],
    input: &[u8],
) -> Result<WasmPluginResult, WasmPluginError> {
    let module = Module::new(engine, module_bytes).map_err(module_error)?;
    let imports = module
        .imports()
        .map(|import| format!("{}::{}", import.module(), import.name()))
        .collect::<Vec<_>>();
    if !imports.is_empty() {
        return Err(WasmPluginError::ForbiddenImports(imports));
    }
    let limits = StoreLimitsBuilder::new()
        .memory_size(manifest.maximum_memory_bytes)
        .memories(1)
        .tables(1)
        .instances(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(engine, StoreState { limits });
    store.limiter(|state| &mut state.limits);
    store.set_fuel(manifest.fuel).map_err(runtime_error)?;
    store.set_epoch_deadline(1);
    store.epoch_deadline_trap();
    let instance = Instance::new(&mut store, &module, &[]).map_err(runtime_error)?;
    let memory = instance
        .get_memory(&mut store, ABI_MEMORY)
        .ok_or_else(|| WasmPluginError::Abi(format!("missing export '{ABI_MEMORY}'")))?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, ABI_ALLOC)
        .map_err(|error| WasmPluginError::Abi(format!("invalid '{ABI_ALLOC}': {error}")))?;
    let invoke = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, ABI_INVOKE)
        .map_err(|error| WasmPluginError::Abi(format!("invalid '{ABI_INVOKE}': {error}")))?;
    let input_len = i32::try_from(input.len())
        .map_err(|_| WasmPluginError::Abi("input length exceeds i32 ABI".into()))?;
    let input_ptr = alloc.call(&mut store, input_len).map_err(runtime_error)?;
    let input_offset = pointer_to_usize(input_ptr)?;
    memory
        .write(&mut store, input_offset, input)
        .map_err(|error| WasmPluginError::Abi(format!("failed to write guest input: {error}")))?;
    let packed = invoke
        .call(&mut store, (input_ptr, input_len))
        .map_err(runtime_error)?;
    let (output_ptr, output_len) = unpack_result(packed)?;
    if output_len > manifest.maximum_output_bytes {
        return Err(WasmPluginError::OutputTooLarge {
            actual: output_len,
            limit: manifest.maximum_output_bytes,
        });
    }
    let mut output = vec![0; output_len];
    memory
        .read(&store, output_ptr, &mut output)
        .map_err(|error| WasmPluginError::Abi(format!("failed to read guest output: {error}")))?;
    if let Ok(dealloc) = instance.get_typed_func::<(i32, i32), ()>(&mut store, ABI_DEALLOC) {
        let _ = dealloc.call(&mut store, (input_ptr, input_len));
        let output_ptr_i32 = i32::try_from(output_ptr)
            .map_err(|_| WasmPluginError::Abi("output pointer exceeds i32 ABI".into()))?;
        let output_len_i32 = i32::try_from(output_len)
            .map_err(|_| WasmPluginError::Abi("output length exceeds i32 ABI".into()))?;
        let _ = dealloc.call(&mut store, (output_ptr_i32, output_len_i32));
    }
    let fuel_remaining = store.get_fuel().map_err(runtime_error)?;
    Ok(WasmPluginResult {
        output,
        fuel_consumed: manifest.fuel.saturating_sub(fuel_remaining),
    })
}

fn unpack_result(value: i64) -> Result<(usize, usize), WasmPluginError> {
    let bits = u64::from_ne_bytes(value.to_ne_bytes());
    let pointer = usize::try_from(bits >> 32)
        .map_err(|_| WasmPluginError::Abi("output pointer exceeds host usize".into()))?;
    let length = usize::try_from(bits & u64::from(u32::MAX))
        .map_err(|_| WasmPluginError::Abi("output length exceeds host usize".into()))?;
    Ok((pointer, length))
}

fn pointer_to_usize(value: i32) -> Result<usize, WasmPluginError> {
    usize::try_from(value)
        .map_err(|_| WasmPluginError::Abi("guest returned a negative pointer".into()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_identifier(name: &str, value: &str, maximum: usize) -> Result<(), WasmPluginError> {
    if value.trim().is_empty()
        || value.len() > maximum
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(WasmPluginError::Manifest(format!(
            "{name} must contain 1-{maximum} letters, numbers, dots, hyphens or underscores"
        )));
    }
    Ok(())
}

fn engine_error(error: wasmtime::Error) -> WasmPluginError {
    WasmPluginError::Runtime(error.to_string())
}

fn module_error(error: wasmtime::Error) -> WasmPluginError {
    WasmPluginError::Module(error.to_string())
}

fn runtime_error(error: impl std::fmt::Display) -> WasmPluginError {
    WasmPluginError::Runtime(error.to_string())
}

#[derive(Debug, Error)]
pub enum WasmPluginError {
    #[error("Wasm plugin manifest is invalid: {0}")]
    Manifest(String),
    #[error("Wasm plugin integrity mismatch: expected {expected}, got {actual}")]
    IntegrityMismatch { expected: String, actual: String },
    #[error("Wasm plugin imports forbidden host capabilities: {0:?}")]
    ForbiddenImports(Vec<String>),
    #[error("Wasm plugin input is too large: {actual} bytes, limit {limit}")]
    InputTooLarge { actual: usize, limit: usize },
    #[error("Wasm plugin output is too large: {actual} bytes, limit {limit}")]
    OutputTooLarge { actual: usize, limit: usize },
    #[error("Wasm plugin ABI error: {0}")]
    Abi(String),
    #[error("Wasm module validation failed: {0}")]
    Module(String),
    #[error("Wasm plugin execution failed: {0}")]
    Runtime(String),
    #[error("Wasm plugin I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const ECHO: &str = r#"
        (module
          (memory (export "memory") 1 2)
          (global $next (mut i32) (i32.const 1024))
          (func (export "sessionweft_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            global.get $next
            local.tee $ptr
            local.get $len
            i32.add
            global.set $next
            local.get $ptr)
          (func (export "sessionweft_dealloc") (param i32 i32))
          (func (export "sessionweft_invoke_v1") (param $ptr i32) (param $len i32) (result i64)
            local.get $ptr
            i64.extend_i32_u
            i64.const 32
            i64.shl
            local.get $len
            i64.extend_i32_u
            i64.or))
    "#;

    fn manifest(bytes: &[u8]) -> WasmPluginManifest {
        WasmPluginManifest {
            plugin_id: "echo".into(),
            version: "1.0.0".into(),
            sha256: hex_digest(bytes),
            maximum_memory_bytes: 2 * 65_536,
            maximum_input_bytes: 4_096,
            maximum_output_bytes: 4_096,
            fuel: 100_000,
            timeout_millis: 1_000,
        }
    }

    #[tokio::test]
    async fn executes_import_free_plugin() {
        let bytes = wasmtime::wat2wasm(ECHO).expect("WAT").into_owned();
        let result = PortableWasmSandbox::new()
            .expect("sandbox")
            .invoke_bytes(manifest(&bytes), bytes, b"hello".to_vec())
            .await
            .expect("invoke");
        assert_eq!(result.output, b"hello");
        assert!(result.fuel_consumed > 0);
    }

    #[tokio::test]
    async fn rejects_wasi_and_other_host_imports() {
        let wat = r#"
          (module
            (import "wasi_snapshot_preview1" "fd_write"
              (func $fd_write (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "sessionweft_alloc") (param i32) (result i32) (i32.const 0))
            (func (export "sessionweft_invoke_v1") (param i32 i32) (result i64) (i64.const 0)))
        "#;
        let bytes = wasmtime::wat2wasm(wat).expect("WAT").into_owned();
        let error = PortableWasmSandbox::new()
            .expect("sandbox")
            .invoke_bytes(manifest(&bytes), bytes, Vec::new())
            .await
            .expect_err("import must be rejected");
        assert!(matches!(error, WasmPluginError::ForbiddenImports(_)));
    }

    #[tokio::test]
    async fn fuel_stops_infinite_guest() {
        let wat = r#"
          (module
            (memory (export "memory") 1)
            (func (export "sessionweft_alloc") (param i32) (result i32) (i32.const 0))
            (func (export "sessionweft_invoke_v1") (param i32 i32) (result i64)
              (loop $forever (br $forever))
              (i64.const 0)))
        "#;
        let bytes = wasmtime::wat2wasm(wat).expect("WAT").into_owned();
        let mut limits = manifest(&bytes);
        limits.fuel = 10_000;
        let error = PortableWasmSandbox::new()
            .expect("sandbox")
            .invoke_bytes(limits, bytes, Vec::new())
            .await
            .expect_err("guest must be interrupted");
        assert!(matches!(error, WasmPluginError::Runtime(_)));
    }

    #[tokio::test]
    async fn output_limit_is_enforced_before_allocation() {
        let wat = r#"
          (module
            (memory (export "memory") 1)
            (func (export "sessionweft_alloc") (param i32) (result i32) (i32.const 0))
            (func (export "sessionweft_invoke_v1") (param i32 i32) (result i64)
              (i64.or
                (i64.shl (i64.const 0) (i64.const 32))
                (i64.const 65535))))
        "#;
        let bytes = wasmtime::wat2wasm(wat).expect("WAT").into_owned();
        let error = PortableWasmSandbox::new()
            .expect("sandbox")
            .invoke_bytes(manifest(&bytes), bytes, Vec::new())
            .await
            .expect_err("large output must fail");
        assert!(matches!(error, WasmPluginError::OutputTooLarge { .. }));
    }
}
