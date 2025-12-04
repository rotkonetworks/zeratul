import { ApiPromise, WsProvider } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    console.log('Current block:', (await api.rpc.chain.getHeader()).number.toNumber());

    // Check parachain lifecycle
    const paraLifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('Para lifecycle:', paraLifecycle.toString() || 'Not found');

    // Check parachains list
    const parachains = await api.query.paras.parachains();
    console.log('Active parachains:', parachains.map(p => p.toNumber()));

    // Check upcoming parachains
    const upcomingParas = await api.query.paras.upcomingParasGenesis.keys();
    console.log('Upcoming paras:', upcomingParas.map(k => k.args[0].toNumber()));

    // Check PVF active vote
    const pvfActiveVote = await api.query.paras.pvfActiveVoteMap.entries();
    console.log('PVF active votes:', pvfActiveVote.length);

    // Check actions queue
    const actionsQueue = await api.query.paras.actionsQueue.entries();
    for (const [key, value] of actionsQueue) {
        console.log('Actions queue at session', key.args[0].toNumber(), ':', value.toHuman());
    }

    await api.disconnect();
}

main().catch(console.error);
