# Ligerito WASM Demo with Web Workers

This demo shows Ligerito polynomial commitment running in the browser with **non-blocking computation** using Web Workers (Penumbra pattern).

## Architecture

### Without Workers (OLD - blocks UI)
```
Main Thread: [Generate] → [Prove - BLOCKS 50ms+] → [Verify - BLOCKS 20ms+]
                            ↑ UI freezes here
```

### With Workers (NEW - non-blocking)
```
Main Thread: [Generate] → [Spawn Worker] → [UI stays responsive!]
                               ↓
Worker Thread:            [Prove 50ms] → [Send result back]
                               ↓
Main Thread:              [Receive result] → [Update UI]
```

## Key Implementation Details

**Penumbra Pattern Applied:**
1. **Spawn dedicated worker per task** - simple and effective
2. **Dynamic WASM import inside worker** - keeps main thread free during init
3. **Automatic cleanup** - `worker.terminate()` after completion
4. **Simple message passing** - `postMessage()` / `onmessage`

**Files:**
- `index.html` - Main UI (spawns workers)
- `worker.js` - Web Worker script (loads WASM, runs prove/verify)
- `ligerito.js` + `ligerito_bg.wasm` - WASM module

## Benefits

✅ **UI never freezes** - proving runs in background thread
✅ **Browser handles parallelization** - multiple proofs can run concurrently
✅ **Simple error handling** - worker errors bubble up cleanly
✅ **Automatic resource cleanup** - workers terminate after use

## Testing

```bash
# Server is already running at http://localhost:8080
# Open in browser and test:

1. Click "Generate Random Polynomial" - instant
2. Click "Generate Proof" - UI stays responsive during proving
3. Try interacting with UI while proving - everything works!
4. Click "Verify Proof" - also runs in worker
```

## Performance

- **2^12 (4KB)**: ~50ms prove, ~20ms verify
- **2^20 (4MB)**: ~1-2s prove, ~470ms verify
- **2^24 (64MB)**: ~10-20s prove, ~470ms verify

All run in background without blocking the UI!

## Comparison with Penumbra

**Similar:**
- Spawn-per-task pattern (no pooling)
- Immediate worker termination
- Dynamic WASM import in worker

**Different:**
- Penumbra: Chrome Extension + Offscreen Document (MV3 requirement)
- Us: Direct Web Workers (simpler, works anywhere)
- Penumbra: Large proving keys (100MB+) loaded per action type
- Us: No external keys needed (all compiled in WASM)

## Next Steps

For production client integration:
1. **Worker pooling** (if many concurrent proofs needed)
2. **Progress callbacks** (for long-running proofs)
3. **Transferable objects** (for large polynomial data - zero-copy)
4. **SharedArrayBuffer** (if multiple workers need shared data)

Currently unnecessary - spawn-per-task is perfect for typical usage patterns.
