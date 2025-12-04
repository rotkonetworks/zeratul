import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

const account = '5GYjFw1tqoMkKjLnwB9BmQ7YvaYzyeYSFkmm1igBJVQKL9gU';
const info = await api.query.system.account(account);

console.log('Account info:');
console.log('  Free:', (Number(info.data.free) / 1e10).toFixed(4), 'PAS');
console.log('  Reserved:', (Number(info.data.reserved) / 1e10).toFixed(4), 'PAS');
console.log('  Frozen:', (Number(info.data.frozen) / 1e10).toFixed(4), 'PAS');
console.log('  Nonce:', info.nonce.toString());
console.log('  Consumers:', info.consumers.toString());
console.log('  Providers:', info.providers.toString());

// Get ED
const ed = Number(api.consts.balances.existentialDeposit);
console.log('\nExistential deposit:', (ed / 1e10).toFixed(4), 'PAS');

// The key question: what's the TRANSFERABLE balance?
// Transferable = Free - max(Frozen, ED needed for providers)
const free = Number(info.data.free);
const frozen = Number(info.data.frozen);
const transferable = free - Math.max(frozen, ed);
console.log('Transferable:', (transferable / 1e10).toFixed(4), 'PAS');

// What if there's a lock or hold?
const locks = await api.query.balances.locks(account);
console.log('\nLocks:', locks.length ? locks.toHuman() : 'none');

const holds = await api.query.balances.holds(account);
console.log('Holds:', holds.length ? holds.toHuman() : 'none');

const freezes = await api.query.balances.freezes(account);
console.log('Freezes:', freezes.length ? freezes.toHuman() : 'none');

// Check the registrar storage for our paraIds
console.log('\nParaId 5081 info:', (await api.query.registrar.paras(5081)).toHuman());
console.log('ParaId 5082 info:', (await api.query.registrar.paras(5082)).toHuman());

await api.disconnect();
