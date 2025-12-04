import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.dotters.network');
const api = await ApiPromise.create({ provider });

console.log('=== On-Demand Coretime Info ===\n');

// Check on-demand pallet constants
try {
    const trafficDefaultValue = api.consts.onDemand?.trafficDefaultValue;
    console.log('Traffic default value:', trafficDefaultValue?.toString());
    
    const maxHistoricalRevenue = api.consts.onDemand?.maxHistoricalRevenue;
    console.log('Max historical revenue:', maxHistoricalRevenue?.toString());
    
    const palletId = api.consts.onDemand?.palletId;
    console.log('Pallet ID:', palletId?.toString());
} catch (e) {
    console.log('On-demand constants error:', e.message);
}

// Check current on-demand price/queue
try {
    const spotPrice = await api.query.onDemand?.spotTraffic();
    console.log('\nSpot traffic:', spotPrice?.toString());
    
    const queueStatus = await api.query.onDemand?.paraIdAffinity?.entries();
    console.log('Queue entries:', queueStatus?.length || 0);
} catch (e) {
    console.log('On-demand query error:', e.message);
}

// Check our parachain lifecycle
const lifecycle = await api.query.paras.paraLifecycles(5082);
console.log('\nParaId 5082 lifecycle:', lifecycle.toString());

await api.disconnect();
