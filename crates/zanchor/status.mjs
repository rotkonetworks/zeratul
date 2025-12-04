import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

const lifecycle = await api.query.paras.paraLifecycles(5082);
console.log('ParaId 5082 lifecycle:', lifecycle.toString());

const info = await api.query.registrar.paras(5082);
console.log('Registrar info:', info.toHuman());

await api.disconnect();
