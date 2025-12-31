import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { readFileSync } from 'fs';

async function deploy() {
  // Connect to local node
  const wsProvider = new WsProvider('ws://127.0.0.1:9944');
  const api = await ApiPromise.create({ provider: wsProvider });

  console.log('Connected to', (await api.rpc.system.chain()).toString());

  // Read contract code
  const code = readFileSync('./escrow.polkavm');
  console.log('Contract size:', code.length, 'bytes');

  // Setup keyring with Alice
  const keyring = new Keyring({ type: 'sr25519' });
  const alice = keyring.addFromUri('//Alice');
  console.log('Deploying from:', alice.address);

  // Check Alice balance
  const { data: balance } = await api.query.system.account(alice.address);
  console.log('Alice balance:', balance.free.toString());

  // Deploy using Revive.instantiate_with_code
  // Parameters: value, gas_limit, storage_deposit_limit, code, data, salt
  const value = 0;
  const gasLimit = { refTime: 500_000_000_000n, proofSize: 500_000n };
  const storageDepositLimit = null;  // No limit
  const data = '0x';  // No constructor data
  const salt = null;  // Random salt

  console.log('Deploying contract...');

  const tx = api.tx.revive.instantiateWithCode(
    value,
    gasLimit,
    storageDepositLimit,
    code,
    data,
    salt
  );

  // Submit and wait for result
  return new Promise((resolve, reject) => {
    tx.signAndSend(alice, ({ status, events, dispatchError }) => {
      if (status.isInBlock) {
        console.log('In block:', status.asInBlock.toString());

        // Look for Instantiated event
        for (const { event } of events) {
          if (event.section === 'revive' && event.method === 'Instantiated') {
            const [deployer, contract] = event.data;
            console.log('Contract deployed at:', contract.toString());
          }
          if (event.section === 'system' && event.method === 'ExtrinsicFailed') {
            console.error('Transaction failed');
            if (dispatchError) {
              if (dispatchError.isModule) {
                const decoded = api.registry.findMetaError(dispatchError.asModule);
                console.error('Error:', decoded.name, decoded.docs.join(' '));
              } else {
                console.error('Error:', dispatchError.toString());
              }
            }
          }
        }
      }
      if (status.isFinalized) {
        console.log('Finalized:', status.asFinalized.toString());
        resolve();
      }
    }).catch(reject);
  });
}

deploy()
  .then(() => process.exit(0))
  .catch(err => {
    console.error('Error:', err);
    process.exit(1);
  });
