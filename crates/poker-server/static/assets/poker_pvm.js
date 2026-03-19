/* @ts-self-types="./poker_pvm.d.ts" */

export class WasmGame {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmGameFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmgame_free(ptr, 0);
    }
    /**
     * @returns {number}
     */
    acting_seat() {
        const ret = wasm.wasmgame_acting_seat(this.__wbg_ptr);
        return ret;
    }
    /**
     * returns: [valid, hand_over, winner, payout, advance_phase] as packed u32
     * apply action. pass seq=0 to auto-assign sequence.
     * returns [valid, hand_over, winner, payout, advance_phase]
     * @param {number} seat
     * @param {number} action
     * @param {number} amount
     * @param {number} seq
     * @returns {Uint32Array}
     */
    apply_action(seat, action, amount, seq) {
        const ret = wasm.wasmgame_apply_action(this.__wbg_ptr, seat, action, amount, seq);
        var v1 = getArrayU32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * apply action with error message returned
     * @param {number} seat
     * @param {number} action
     * @param {number} amount
     * @param {number} seq
     * @returns {string}
     */
    apply_action_debug(seat, action, amount, seq) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmgame_apply_action_debug(this.__wbg_ptr, seat, action, amount, seq);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @param {number} seat
     * @returns {number}
     */
    bet(seat) {
        const ret = wasm.wasmgame_bet(this.__wbg_ptr, seat);
        return ret >>> 0;
    }
    /**
     * @returns {number}
     */
    button() {
        const ret = wasm.wasmgame_button(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {number}
     */
    community_count() {
        const ret = wasm.wasmgame_community_count(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {number} a0
     * @param {number} a1
     * @param {number} b0
     * @param {number} b1
     * @param {number} c0
     * @param {number} c1
     * @param {number} c2
     * @param {number} c3
     * @param {number} c4
     */
    deal(a0, a1, b0, b1, c0, c1, c2, c3, c4) {
        wasm.wasmgame_deal(this.__wbg_ptr, a0, a1, b0, b1, c0, c1, c2, c3, c4);
    }
    /**
     * @returns {string}
     */
    debug_state() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmgame_debug_state(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @returns {number}
     */
    hand_number() {
        const ret = wasm.wasmgame_hand_number(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @param {number} buyin
     * @param {number} small_blind
     * @param {number} big_blind
     */
    constructor(buyin, small_blind, big_blind) {
        const ret = wasm.wasmgame_new(buyin, small_blind, big_blind);
        this.__wbg_ptr = ret >>> 0;
        WasmGameFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @returns {number}
     */
    phase() {
        const ret = wasm.wasmgame_phase(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {number}
     */
    pot() {
        const ret = wasm.wasmgame_pot(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @returns {number}
     */
    round_actions() {
        const ret = wasm.wasmgame_round_actions(this.__wbg_ptr);
        return ret;
    }
    /**
     * set stacks and button for next hand (sync guest with host)
     * @param {number} stack0
     * @param {number} stack1
     * @param {number} btn
     */
    set_state(stack0, stack1, btn) {
        wasm.wasmgame_set_state(this.__wbg_ptr, stack0, stack1, btn);
    }
    /**
     * @returns {number}
     */
    showdown() {
        const ret = wasm.wasmgame_showdown(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {number} seat
     * @returns {number}
     */
    stack(seat) {
        const ret = wasm.wasmgame_stack(this.__wbg_ptr, seat);
        return ret >>> 0;
    }
    /**
     * update community cards without resetting hand state.
     * used when shuffle reveals cards incrementally.
     * @param {number} c0
     * @param {number} c1
     * @param {number} c2
     * @param {number} c3
     * @param {number} c4
     */
    update_community(c0, c1, c2, c3, c4) {
        wasm.wasmgame_update_community(this.__wbg_ptr, c0, c1, c2, c3, c4);
    }
    /**
     * update opponent's hole cards (for showdown eval on host side)
     * @param {number} c0
     * @param {number} c1
     */
    update_opp_cards(c0, c1) {
        wasm.wasmgame_update_opp_cards(this.__wbg_ptr, c0, c1);
    }
}
if (Symbol.dispose) WasmGame.prototype[Symbol.dispose] = WasmGame.prototype.free;

function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_throw_6ddd609b62940d55: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./poker_pvm_bg.js": import0,
    };
}

const WasmGameFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmgame_free(ptr >>> 0, 1));

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('poker_pvm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
