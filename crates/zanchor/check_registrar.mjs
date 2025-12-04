import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const PARA_ID = 5082;

async function main() {
    const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
    const api = await ApiPromise.create({ provider });

    console.log('=== Check Registrar Options for Para 5082 ===\n');

    // Check registrar pallet transactions
    const registrarTxs = Object.keys(api.tx.registrar || {});
    console.log('Registrar txs available:', registrarTxs);

    // Check registrar queries
    const registrarQueries = Object.keys(api.query.registrar || {});
    console.log('Registrar queries:', registrarQueries);

    // Check who manages this para
    const paras = await api.query.registrar.paras(PARA_ID);
    console.log('\nPara info from registrar:');
    console.log(paras.toHuman());

    // Check if there's a manager key we control
    const keyring = new Keyring({ type: 'sr25519' });
    const alice = keyring.addFromUri('//Alice');
    const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');

    console.log('\nOur accounts:');
    console.log('  Alice:', alice.address);
    console.log('  Signer:', signer.address);

    // Check paras pallet
    const parasTxs = Object.keys(api.tx.paras || {});
    console.log('\nParas txs available:', parasTxs);

    // Check if we have any way to schedule code upgrade via paras
    const parasQueries = Object.keys(api.query.paras || {});
    console.log('Paras queries:', parasQueries);

    // Check current head and code
    const currentHead = await api.query.paras.heads(PARA_ID);
    const currentCodeHash = await api.query.paras.currentCodeHash(PARA_ID);
    console.log('\nCurrent para state:');
    console.log('  Head:', currentHead.toHex().slice(0, 66) + '...');
    console.log('  Code hash:', currentCodeHash.toHuman());

    // Check lifecycle
    const lifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('  Lifecycle:', lifecycle.toHuman());

    // Check if registrar.deregister exists and what it needs
    if (api.tx.registrar.deregister) {
        console.log('\n=== registrar.deregister exists ===');
        console.log('Method info:', api.tx.registrar.deregister.meta.toHuman());
    }

    // Check if registrar.scheduleCodeUpgrade exists
    if (api.tx.registrar.scheduleCodeUpgrade) {
        console.log('\n=== registrar.scheduleCodeUpgrade exists ===');
        console.log('Method info:', api.tx.registrar.scheduleCodeUpgrade.meta.toHuman());
    }

    // Check if there's addLock/removeLock for para manager
    if (api.tx.registrar.addLock) {
        console.log('\n=== registrar.addLock exists ===');
    }

    // Check pending swaps
    const pendingSwap = await api.query.registrar.pendingSwap(PARA_ID);
    console.log('\nPending swap:', pendingSwap.toHuman());

    // Check hrmp channels
    const hrmpQueries = Object.keys(api.query.hrmp || {});
    console.log('\nHRMP queries:', hrmpQueries);

    // Can we use registrar.swap to essentially reset?
    if (api.tx.registrar.swap) {
        console.log('\n=== registrar.swap exists ===');
        console.log('Method info:', api.tx.registrar.swap.meta.toHuman());
    }

    // Check if forceSetCurrentHead exists in paras
    if (api.tx.paras && api.tx.paras.forceSetCurrentHead) {
        console.log('\n=== paras.forceSetCurrentHead exists (requires root) ===');
    }

    // Check sudo pallet
    const sudoKey = await api.query.sudo?.key?.();
    if (sudoKey) {
        console.log('\nSudo key on Paseo:', sudoKey.toHuman());
    } else {
        console.log('\nNo sudo pallet or key found on Paseo');
    }

    // Try to build a deregister call to see if we can sign it
    console.log('\n=== Attempting to build deregister call ===');
    try {
        const deregisterCall = api.tx.registrar.deregister(PARA_ID);
        console.log('Call hex:', deregisterCall.toHex());
        console.log('This call would need to be signed by the para manager');
    } catch (e) {
        console.log('Error building call:', e.message);
    }

    await api.disconnect();
}

main().catch(console.error);
