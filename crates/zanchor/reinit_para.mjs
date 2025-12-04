import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const PARA_ID = 5082;

async function main() {
    const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
    const api = await ApiPromise.create({ provider });

    console.log('=== Re-initialize Parachain ===');
    console.log('ParaId:', PARA_ID);

    // Read genesis data
    const genesisState = fs.readFileSync('/tmp/zanchor-genesis-state.hex', 'utf8').trim();
    const genesisWasm = fs.readFileSync('/tmp/zanchor-genesis-wasm.hex', 'utf8').trim();

    console.log('Genesis state length:', genesisState.length);
    console.log('Genesis WASM length:', genesisWasm.length);

    // Check current para state
    const lifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('Current lifecycle:', lifecycle.toHuman());

    const currentHead = await api.query.paras.heads(PARA_ID);
    console.log('Current head:', currentHead.toHex().slice(0, 66) + '...');

    // For re-initialization, we need sudo on Paseo
    // The call would be: parasSudoWrapper.sudoScheduleParaInitialize
    // But we likely don't have sudo access on Paseo testnet

    // Alternative: Check if we can schedule a code upgrade using our sudo on the parachain
    // Or use paraSudoWrapper if available

    console.log('\n=== Required Action ===');
    console.log('To re-initialize para with correct genesis, use:');
    console.log('1. parasSudoWrapper.sudoScheduleParaInitialize(5082, {genesisHead, validationCode, paraKind: Parathread})');
    console.log('2. First need to schedule para cleanup: parasSudoWrapper.sudoScheduleParaCleanup(5082)');
    console.log('3. Or use registrar.forceRegister if you have sudo');

    // Check if we can at least do a runtime upgrade to fix the paraId in-place
    console.log('\n=== Alternative: Force Code Upgrade ===');
    console.log('If para can produce at least one block, could do runtime upgrade to fix paraId');
    console.log('But this is chicken-and-egg since it cannot produce blocks with wrong paraId');

    // Let's check if there's any way via registrar
    const registrarQueries = Object.keys(api.query.registrar || {});
    console.log('\nRegistrar queries available:', registrarQueries);

    const parasSudoQueries = Object.keys(api.tx.parasSudoWrapper || {});
    console.log('ParasSudoWrapper txs available:', parasSudoQueries);

    await api.disconnect();
}

main().catch(console.error);
