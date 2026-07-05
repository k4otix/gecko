use wasmtime::*;
use wasmtime_wasi::{WasiCtxBuilder, WasiCtx};
use wasmtime_wasi::pipe::{MemoryReadPipe, MemoryWritePipe};

fn test_wasi() {
    let engine = Engine::default();
    let mut linker = Linker::<WasiCtx>::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |s| s).unwrap();
    
    let stdin = MemoryReadPipe::new("test".as_bytes());
    let stdout = MemoryWritePipe::new(1024);
    
    let mut wasi = WasiCtxBuilder::new();
    wasi.stdin(Box::new(stdin));
    wasi.stdout(Box::new(stdout));
    
    let mut store = Store::new(&engine, wasi.build());
}
