import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

const account = '5GYjFw1tqoMkKjLnwB9BmQ7YvaYzyeYSFkmm1igBJVQKL9gU';
const info = await api.query.system.account(account);

console.log('Account:', account);
console.log('Free:', (Number(info.data.free) / 10000000000).toFixed(4), 'PAS');
console.log('Reserved:', (Number(info.data.reserved) / 10000000000).toFixed(4), 'PAS');
console.log('Frozen:', (Number(info.data.frozen) / 10000000000).toFixed(4), 'PAS');

// Check existential deposit
const ed = api.consts.balances.existentialDeposit;
console.log('Existential deposit:', (Number(ed) / 10000000000).toFixed(4), 'PAS');

// Calculate what we need
console.log('\n=== Deposit calculation ===');
const paraDeposit = 100; // PAS
const dataBytes = 1241189 + 99; // bytes
const dataDepositRate = 0.001; // PAS per byte
const dataDeposit = dataBytes * dataDepositRate;
const totalDeposit = paraDeposit + dataDeposit;
console.log('Para deposit:', paraDeposit, 'PAS');
console.log('Data deposit:', dataDeposit.toFixed(4), 'PAS');
console.log('Total deposit:', totalDeposit.toFixed(4), 'PAS');
console.log('Estimated fee:', 124, 'PAS');
console.log('Total needed:', (totalDeposit + 124).toFixed(4), 'PAS');

await api.disconnect();
