import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

console.log('=== ParaId 5082 Registration Status ===\n');

// Check registrar paras
const parasInfo = await api.query.registrar.paras(5082);
console.log('Registrar paras info:', parasInfo.toHuman());

// Check paras lifecycle
const lifecycle = await api.query.paras.paraLifecycles(5082);
console.log('Paras lifecycle:', lifecycle.toString() || 'not found');

// Check PVF active vote
const pvfActiveVote = await api.query.paras.pvfActiveVoteMap(5082);
console.log('PVF active vote:', pvfActiveVote.toString() || 'none');

// Check account balance
const account = '5GYjFw1tqoMkKjLnwB9BmQ7YvaYzyeYSFkmm1igBJVQKL9gU';
const info = await api.query.system.account(account);
console.log('\nAccount balance:');
console.log('  Free:', (Number(info.data.free) / 1e10).toFixed(4), 'PAS');
console.log('  Reserved:', (Number(info.data.reserved) / 1e10).toFixed(4), 'PAS');

await api.disconnect();
