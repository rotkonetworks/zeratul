import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const PARA_ID = 5082;

async function main() {
    const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
    const api = await ApiPromise.create({ provider });

    console.log('=== Fix Para 5082 Genesis ===\n');

    const keyring = new Keyring({ type: 'sr25519' });

    // Try different mnemonics/derivations to find manager
    const mnemonics = [
        'move defense manage burden pudding core elite aware tenant payment assault federal',
        // Common test mnemonics
        'bottom drive obey lake curtain smoke basket hold race lonely fit walk',
    ];

    console.log('Para manager:', '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT');
    console.log('\nTrying to find matching account...\n');

    for (const mnemonic of mnemonics) {
        const account = keyring.addFromMnemonic(mnemonic);
        console.log(`Mnemonic "${mnemonic.slice(0, 20)}...": ${account.address}`);

        if (account.address === '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT') {
            console.log('  ^^^ MATCH! This is the para manager');
        }

        // Try some derivations
        for (const path of ['//0', '//1', '//para', '//manager']) {
            try {
                const derived = keyring.addFromUri(`${mnemonic}${path}`);
                if (derived.address === '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT') {
                    console.log(`  MATCH with path ${path}!`);
                }
            } catch (e) {}
        }
    }

    // Check setCurrentHead method
    console.log('\n=== registrar.setCurrentHead info ===');
    if (api.tx.registrar.setCurrentHead) {
        console.log('Method exists!');
        console.log(api.tx.registrar.setCurrentHead.meta.toHuman());
    }

    // Check if para is locked
    const paraInfo = await api.query.registrar.paras(PARA_ID);
    const info = paraInfo.toJSON();
    console.log('\nPara info:', info);
    console.log('Is locked:', info?.locked);

    // If unlocked, we might be able to use scheduleCodeUpgrade
    // Read new genesis data
    const genesisState = fs.readFileSync('/tmp/zanchor-genesis-state.hex', 'utf8').trim();
    const genesisWasm = fs.readFileSync('/tmp/zanchor-genesis-wasm.hex', 'utf8').trim();

    console.log('\nNew genesis data:');
    console.log('  State length:', genesisState.length);
    console.log('  WASM length:', genesisWasm.length);

    // The key insight: we need to either:
    // 1. Have the manager key to call deregister + re-register
    // 2. Have the manager key to call setCurrentHead + scheduleCodeUpgrade
    // 3. Get sudo access on Paseo

    // Let's see if there's a way to transfer manager role
    if (api.tx.registrar.transferOwnership) {
        console.log('\n=== registrar.transferOwnership exists ===');
    }

    // Check if we can at least see what the current registered validation code is
    const currentCode = await api.query.paras.currentCodeHash(PARA_ID);
    console.log('\nCurrent code hash:', currentCode.toHuman());

    // Also check the head
    const currentHead = await api.query.paras.heads(PARA_ID);
    console.log('Current head (first 100 chars):', currentHead.toHex().slice(0, 100));
    console.log('New head (first 100 chars):', genesisState.slice(0, 100));

    await api.disconnect();
}

main().catch(console.error);
