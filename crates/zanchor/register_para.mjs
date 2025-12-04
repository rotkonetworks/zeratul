import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

console.log('Connected to Paseo');

// Create keyring and signer
const keyring = new Keyring({ type: 'sr25519' });
const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');
console.log('Signer address:', signer.address);

// Check balance
const accountInfo = await api.query.system.account(signer.address);
console.log('Free balance:', accountInfo.data.free.toString(), 'planck =', Number(accountInfo.data.free) / 10000000000, 'PAS');
console.log('Reserved balance:', accountInfo.data.reserved.toString(), 'planck');

// Read genesis files
const genesisHeadHex = fs.readFileSync('/home/alice/rotko/zeratul/crates/zanchor/zanchor-genesis-head.hex', 'utf8').trim();
const validationCodeHex = fs.readFileSync('/home/alice/rotko/zeratul/crates/zanchor/zanchor-genesis-wasm.hex', 'utf8').trim();

const genesisHead = genesisHeadHex.startsWith('0x') ? genesisHeadHex : '0x' + genesisHeadHex;
const validationCode = validationCodeHex.startsWith('0x') ? validationCodeHex : '0x' + validationCodeHex;

console.log('Genesis head length:', (genesisHead.length - 2) / 2, 'bytes');
console.log('Validation code length:', (validationCode.length - 2) / 2, 'bytes');

// Calculate expected deposit
const paraDeposit = 1000000000000n; // 100 PAS
const dataDepositPerByte = 10000000n; // 0.001 PAS
const dataBytes = BigInt((genesisHead.length - 2) / 2 + (validationCode.length - 2) / 2);
const expectedDeposit = paraDeposit + (dataBytes * dataDepositPerByte);
console.log('Expected deposit:', expectedDeposit.toString(), 'planck =', Number(expectedDeposit) / 10000000000, 'PAS');

// Create the registration tx
const paraId = 5082;
console.log('\nAttempting to register ParaId:', paraId);

const tx = api.tx.registrar.register(paraId, genesisHead, validationCode);

// Get payment info (fee estimate)
const paymentInfo = await tx.paymentInfo(signer);
console.log('Estimated fee:', paymentInfo.partialFee.toString(), 'planck =', Number(paymentInfo.partialFee) / 10000000000, 'PAS');

// Try dry run first
console.log('\nDoing dry run...');
const dryRunResult = await api.rpc.system.dryRun(tx.toHex());
console.log('Dry run result:', dryRunResult.toString());

// If dry run OK, submit
if (dryRunResult.isOk) {
    console.log('\nSubmitting transaction...');
    const hash = await tx.signAndSend(signer);
    console.log('Transaction hash:', hash.toHex());
} else {
    console.log('\nDry run failed, not submitting');
}

await api.disconnect();
