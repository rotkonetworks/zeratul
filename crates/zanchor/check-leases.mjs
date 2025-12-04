import { ApiPromise, WsProvider } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    const currentBlock = (await api.rpc.chain.getHeader()).number.toNumber();
    console.log('Current block:', currentBlock);

    // Check lease info
    const leases = await api.query.slots.leases(PARA_ID);
    console.log('Leases for para 2000:', leases.toHuman());

    // Check lease period length
    const leasePeriod = api.consts.slots?.leasePeriod?.toNumber() || 100;
    console.log('Lease period length:', leasePeriod, 'blocks');

    const currentLeasePeriod = Math.floor(currentBlock / leasePeriod);
    console.log('Current lease period:', currentLeasePeriod);
    console.log('Next period starts at block:', (currentLeasePeriod + 1) * leasePeriod);

    // Check actions queue
    const actionsQueue = await api.query.paras.actionsQueue.entries();
    for (const [key, value] of actionsQueue) {
        console.log('Actions queue at session', key.args[0].toNumber(), ':', value.toHuman());
    }

    // Check lifecycle
    const lifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('Para lifecycle:', lifecycle.toString());

    await api.disconnect();
}

main().catch(console.error);
