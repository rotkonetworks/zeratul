# zanchor asset minting design

adapted from interlay's issue/redeem pallets for frost-based custody.

## key difference from interlay

interlay uses a **vault model** where individual vaults:
- lock collateral
- receive btc deposits to their own addresses
- are responsible for individual redemptions
- can be liquidated independently

zanchor uses a **collective custody model** where:
- all signers share one frost-derived custody address
- deposits go to the collective address
- withdrawals are batched into checkpoints
- slashing is for liveness, not collateral liquidation

## what to take from interlay

### 1. amount wrapper (currency/src/amount.rs)

the `Amount<T>` type is excellent - wraps balance + currency_id with:
- `mint_to(account)` - uses orml_tokens::deposit
- `burn_from(account)` - uses orml_tokens::slash_reserved
- `lock_on(account)` - uses orml_tokens::reserve
- `unlock_on(account)` - uses orml_tokens::unreserve
- `transfer(source, dest)` - uses orml_tokens::transfer

```rust
pub fn burn_from(&self, account_id: &T::AccountId) -> DispatchResult {
    ensure!(
        <orml_tokens::Pallet<T>>::slash_reserved(self.currency_id, account_id, self.amount).is_zero(),
        orml_tokens::Error::<T>::BalanceTooLow
    );
    Ok(())
}

pub fn mint_to(&self, account_id: &T::AccountId) -> DispatchResult {
    <orml_tokens::Pallet<T>>::deposit(self.currency_id, account_id, self.amount)
}
```

this is the core minting/burning logic - just wraps orml_tokens.

### 2. secure id generation (security/src/lib.rs)

```rust
pub fn get_secure_id(id: &T::AccountId) -> H256 {
    let mut hasher = Sha256::default();
    hasher.input(id.encode());
    hasher.input(Self::get_nonce().encode());
    hasher.input(frame_system::Pallet::<T>::parent_hash());
    let mut result = [0; 32];
    result.copy_from_slice(&hasher.result()[..]);
    H256(result)
}
```

generates unique request ids that can't be replayed even after purge-chain.

### 3. active block counting

```rust
#[pallet::storage]
pub type ActiveBlockCount<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;
```

tracks blocks only when parachain is running. used for fair expiry calculations.

## simplified deposit flow for zanchor

no vaults, no griefing collateral, no per-vault btc addresses.

```rust
#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct DepositRequest<AccountId, BlockNumber, Balance> {
    pub id: H256,
    pub depositor: AccountId,
    pub amount: Balance,
    pub btc_txid: Option<H256Le>,
    pub created_at: BlockNumber,
    pub confirmed_at: Option<BlockNumber>,
    pub status: DepositStatus,
}

#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
pub enum DepositStatus {
    Pending,
    Confirmed,
    Minted,
    Expired,
}
```

### deposit calls

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    /// user requests deposit, gets the collective custody address
    #[pallet::call_index(0)]
    pub fn request_deposit(origin: OriginFor<T>) -> DispatchResult {
        let depositor = ensure_signed(origin)?;

        let deposit_id = ext::security::get_secure_id::<T>(&depositor);

        // all deposits go to same frost custody address
        let custody_address = T::FrostBridge::custody_address();

        let request = DepositRequest {
            id: deposit_id,
            depositor: depositor.clone(),
            amount: Zero::zero(),  // filled on confirmation
            btc_txid: None,
            created_at: ext::security::active_block_number::<T>(),
            confirmed_at: None,
            status: DepositStatus::Pending,
        };

        DepositRequests::<T>::insert(deposit_id, request);

        Self::deposit_event(Event::DepositRequested {
            deposit_id,
            depositor,
            custody_address,
        });

        Ok(())
    }

    /// anyone can submit proof of btc deposit
    #[pallet::call_index(1)]
    pub fn confirm_deposit(
        origin: OriginFor<T>,
        deposit_id: H256,
        unchecked_transaction: FullTransactionProof,
    ) -> DispatchResult {
        let _ = ensure_signed(origin)?;

        let mut request = DepositRequests::<T>::get(deposit_id)
            .ok_or(Error::<T>::DepositNotFound)?;

        ensure!(
            request.status == DepositStatus::Pending,
            Error::<T>::InvalidDepositStatus
        );

        // verify btc transaction via spv
        let (txid, amount) = ext::btc_relay::verify_deposit::<T>(
            unchecked_transaction,
            T::FrostBridge::custody_address(),
        )?;

        // update request
        request.btc_txid = Some(txid);
        request.amount = amount;
        request.confirmed_at = Some(ext::security::active_block_number::<T>());
        request.status = DepositStatus::Confirmed;

        DepositRequests::<T>::insert(deposit_id, request.clone());

        Self::deposit_event(Event::DepositConfirmed {
            deposit_id,
            btc_txid: txid,
            amount,
        });

        Ok(())
    }

    /// mint zbtc after sufficient confirmations
    #[pallet::call_index(2)]
    pub fn mint_zbtc(origin: OriginFor<T>, deposit_id: H256) -> DispatchResult {
        let _ = ensure_signed(origin)?;

        let mut request = DepositRequests::<T>::get(deposit_id)
            .ok_or(Error::<T>::DepositNotFound)?;

        ensure!(
            request.status == DepositStatus::Confirmed,
            Error::<T>::InvalidDepositStatus
        );

        // check btc has enough confirmations
        let btc_txid = request.btc_txid.ok_or(Error::<T>::BtcTxNotSet)?;
        ensure!(
            ext::btc_relay::has_sufficient_confirmations::<T>(btc_txid)?,
            Error::<T>::InsufficientConfirmations
        );

        // mint zbtc to depositor
        let zbtc_amount = Amount::new(request.amount, T::ZbtcCurrencyId::get());
        zbtc_amount.mint_to(&request.depositor)?;

        request.status = DepositStatus::Minted;
        DepositRequests::<T>::insert(deposit_id, request.clone());

        Self::deposit_event(Event::ZbtcMinted {
            deposit_id,
            depositor: request.depositor,
            amount: request.amount,
        });

        Ok(())
    }
}
```

## simplified withdrawal flow

batched into checkpoints, no per-vault handling.

```rust
#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct WithdrawalRequest<AccountId, BlockNumber, Balance> {
    pub id: H256,
    pub requester: AccountId,
    pub btc_dest: BtcAddress,
    pub amount: Balance,
    pub fee: Balance,
    pub created_at: BlockNumber,
    pub checkpoint_id: Option<u64>,
    pub status: WithdrawalStatus,
}

#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
pub enum WithdrawalStatus {
    Pending,
    InCheckpoint,
    Completed,
    Cancelled,
}
```

### withdrawal calls

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    /// request btc withdrawal - locks and burns zbtc immediately
    #[pallet::call_index(10)]
    pub fn request_withdrawal(
        origin: OriginFor<T>,
        btc_dest: BtcAddress,
        amount: BalanceOf<T>,
    ) -> DispatchResult {
        let requester = ensure_signed(origin)?;

        // validate btc address
        ensure!(!btc_dest.is_zero(), Error::<T>::InvalidBtcAddress);

        let zbtc_amount = Amount::new(amount, T::ZbtcCurrencyId::get());

        // check balance
        ensure!(
            ext::currency::get_free_balance::<T>(&requester, T::ZbtcCurrencyId::get())
                .ge(&zbtc_amount)?,
            Error::<T>::InsufficientBalance
        );

        // calculate fee
        let fee = ext::fee::get_withdrawal_fee::<T>(&zbtc_amount)?;
        let net_amount = zbtc_amount.checked_sub(&fee)?;

        // ensure above dust
        ensure!(
            net_amount.ge(&Self::dust_value())?,
            Error::<T>::AmountBelowDust
        );

        // lock the tokens
        zbtc_amount.lock_on(&requester)?;

        // burn immediately (unlike interlay which waits for btc proof)
        // we trust the signers to process the checkpoint
        zbtc_amount.burn_from(&requester)?;

        let withdrawal_id = ext::security::get_secure_id::<T>(&requester);

        let request = WithdrawalRequest {
            id: withdrawal_id,
            requester: requester.clone(),
            btc_dest,
            amount: net_amount.amount(),
            fee: fee.amount(),
            created_at: ext::security::active_block_number::<T>(),
            checkpoint_id: None,
            status: WithdrawalStatus::Pending,
        };

        WithdrawalRequests::<T>::insert(withdrawal_id, request);
        PendingWithdrawals::<T>::mutate(|v| v.push(withdrawal_id));

        // distribute fee
        fee.mint_to(&T::FeePool::get())?;
        ext::fee::distribute_rewards::<T>(&fee)?;

        Self::deposit_event(Event::WithdrawalRequested {
            withdrawal_id,
            requester,
            btc_dest,
            amount: net_amount.amount(),
            fee: fee.amount(),
        });

        Ok(())
    }
}
```

## checkpoint creation

batches pending withdrawals into a single bitcoin transaction.

```rust
#[pallet::hooks]
impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
    fn on_finalize(n: BlockNumberFor<T>) {
        // create checkpoint if conditions met
        if Self::should_create_checkpoint(n) {
            let _ = Self::create_checkpoint(n);
        }
    }
}

impl<T: Config> Pallet<T> {
    fn should_create_checkpoint(n: BlockNumberFor<T>) -> bool {
        let pending = PendingWithdrawals::<T>::get();
        let last_checkpoint = LastCheckpointBlock::<T>::get();

        // checkpoint if:
        // 1. pending > MAX_PENDING threshold, or
        // 2. time since last checkpoint > MAX_INTERVAL
        pending.len() >= T::MaxPendingWithdrawals::get() as usize ||
            n.saturating_sub(last_checkpoint) >= T::MaxCheckpointInterval::get()
    }

    fn create_checkpoint(n: BlockNumberFor<T>) -> DispatchResult {
        let pending = PendingWithdrawals::<T>::take();
        if pending.is_empty() {
            return Ok(());
        }

        let checkpoint_id = NextCheckpointId::<T>::mutate(|id| {
            *id += 1;
            *id
        });

        // collect withdrawal info
        let mut outputs: Vec<(BtcAddress, Balance)> = Vec::new();
        let mut total_amount = Zero::zero();

        for withdrawal_id in pending.iter() {
            if let Some(mut request) = WithdrawalRequests::<T>::get(withdrawal_id) {
                outputs.push((request.btc_dest, request.amount));
                total_amount = total_amount.saturating_add(request.amount);

                request.checkpoint_id = Some(checkpoint_id);
                request.status = WithdrawalStatus::InCheckpoint;
                WithdrawalRequests::<T>::insert(withdrawal_id, request);
            }
        }

        // construct bitcoin transaction
        let btc_tx = Self::construct_checkpoint_tx(outputs, total_amount)?;

        // request frost signing
        T::FrostBridge::request_signature(
            checkpoint_id,
            btc_tx.encode(),
            n + T::SigningDeadline::get(),
        )?;

        let checkpoint = Checkpoint {
            id: checkpoint_id,
            withdrawals: pending,
            btc_tx_unsigned: btc_tx,
            created_at: n,
            signed_at: None,
            broadcast_at: None,
            confirmed_at: None,
            status: CheckpointStatus::PendingSignature,
        };

        Checkpoints::<T>::insert(checkpoint_id, checkpoint);
        LastCheckpointBlock::<T>::put(n);

        Self::deposit_event(Event::CheckpointCreated {
            checkpoint_id,
            withdrawal_count: pending.len() as u32,
            total_amount,
        });

        Ok(())
    }
}
```

## difference summary

| aspect | interlay | zanchor |
|--------|----------|---------|
| custody | individual vaults | collective frost |
| deposit address | per-vault | single shared |
| collateral | vaults lock collateral | signers stake for liveness |
| withdrawal | vault-by-vault | batched checkpoints |
| burn timing | after btc proof | immediate on request |
| recovery | vault replacement | emergency governance |
| slashing | undercollateralization | signing failures |

## crates to use from interlay

1. **bitcoin** - btc primitives (H256Le, merkle proofs, script parsing)
2. **btc-relay** - spv light client, header storage, tx verification
3. **currency::Amount** - wrapper for orml_tokens operations
4. **security** - secure id generation, active block counting

## crates NOT needed

1. **vault-registry** - no vault model
2. **oracle** - no collateral pricing needed
3. **fee** - simplified, could be simpler
4. **loans** - no lending
5. **nomination** - no vault nomination
6. **replace** - no vault replacement
7. **staking** - use substrate's built-in

## dependencies for pallet-custody

```toml
[dependencies]
# from interlay (or adapted)
bitcoin = { path = "../bitcoin", default-features = false }
btc-relay = { path = "../btc-relay", default-features = false }

# standard orml for token operations
orml-tokens = { default-features = false }
orml-traits = { default-features = false }

# substrate
frame-support = { default-features = false }
frame-system = { default-features = false }
sp-runtime = { default-features = false }
sp-std = { default-features = false }
sp-core = { default-features = false }

# local
pallet-frost-bridge = { path = "../frost-bridge", default-features = false }
```
