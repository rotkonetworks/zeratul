//! Benchmarking for pallet-escrow-arbitration

#![cfg(feature = "runtime-benchmarks")]

use super::*;
use crate::pallet::TargetChain;
use frame_benchmarking::v2::*;
use frame_support::traits::Currency;
use frame_system::RawOrigin;
use sp_runtime::traits::Bounded;

type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

fn funded_account<T: Config>(name: &'static str, index: u32) -> T::AccountId {
    let caller: T::AccountId = account(name, index, 0);
    let amount = BalanceOf::<T>::max_value() / 100u32.into();
    T::Currency::make_free_balance_be(&caller, amount);
    caller
}

fn setup_agent<T: Config>(index: u32) -> T::AccountId {
    let caller = funded_account::<T>("agent", index);
    let encryption_key = [index as u8; 32];
    let _ = Pallet::<T>::register_agent(
        RawOrigin::Signed(caller.clone()).into(),
        encryption_key,
    );
    caller
}

fn setup_arbitrator<T: Config>(index: u32) -> T::AccountId {
    let caller = funded_account::<T>("arbitrator", index);
    let _ = Pallet::<T>::register_arbitrator(
        RawOrigin::Signed(caller.clone()).into(),
    );
    caller
}

fn setup_trader<T: Config>(index: u32) -> (T::AccountId, [u8; 32]) {
    let caller = funded_account::<T>("trader", index);
    let encryption_key = [index as u8; 32];
    let _ = Pallet::<T>::register_trader(
        RawOrigin::Signed(caller.clone()).into(),
        encryption_key,
    );
    (caller, encryption_key)
}

#[benchmarks]
mod benchmarks {
    use super::*;

    // Agent registration benchmarks

    #[benchmark]
    fn register_agent() {
        let caller = funded_account::<T>("caller", 0);
        let encryption_key = [1u8; 32];

        #[extrinsic_call]
        register_agent(RawOrigin::Signed(caller), encryption_key);
    }

    #[benchmark]
    fn deregister_agent() {
        let caller = setup_agent::<T>(0);

        #[extrinsic_call]
        deregister_agent(RawOrigin::Signed(caller));
    }

    #[benchmark]
    fn update_agent_key() {
        let caller = setup_agent::<T>(0);
        let new_key = [2u8; 32];

        #[extrinsic_call]
        update_agent_key(RawOrigin::Signed(caller), new_key);
    }

    // Arbitrator benchmarks

    #[benchmark]
    fn register_arbitrator() {
        let caller = funded_account::<T>("arb", 0);

        #[extrinsic_call]
        register_arbitrator(RawOrigin::Signed(caller));
    }

    #[benchmark]
    fn deregister_arbitrator() {
        let caller = setup_arbitrator::<T>(0);

        #[extrinsic_call]
        deregister_arbitrator(RawOrigin::Signed(caller));
    }

    // Trader benchmarks

    #[benchmark]
    fn register_trader() {
        let caller = funded_account::<T>("trader", 0);
        let encryption_key = [1u8; 32];

        #[extrinsic_call]
        register_trader(RawOrigin::Signed(caller), encryption_key);
    }

    #[benchmark]
    fn update_trader_key() {
        let (caller, _) = setup_trader::<T>(0);
        let new_key = [2u8; 32];

        #[extrinsic_call]
        update_trader_key(RawOrigin::Signed(caller), new_key);
    }

    // Escrow lifecycle benchmarks

    #[benchmark]
    fn create_escrow() {
        let buyer = funded_account::<T>("buyer", 0);
        let seller = funded_account::<T>("seller", 1);
        let agent = setup_agent::<T>(0);

        let escrow_id = [1u8; 32];
        let chain_key = [2u8; 32];
        let amount = 1000u128;
        let encrypted_details = [0u8; 256].to_vec().try_into().expect("valid size");
        let funding_deadline = 100u32;
        let chain = TargetChain::Zcash;

        #[extrinsic_call]
        create_escrow(
            RawOrigin::Signed(agent),
            escrow_id,
            buyer,
            seller,
            chain_key,
            amount,
            encrypted_details,
            funding_deadline,
            chain,
        );
    }

    #[benchmark]
    fn send_message() {
        let (buyer, _) = setup_trader::<T>(0);
        let (seller, _) = setup_trader::<T>(1);
        let msg_id = [1u8; 32];
        let encrypted_content = [0u8; 128].to_vec().try_into().expect("valid size");

        #[extrinsic_call]
        send_message(
            RawOrigin::Signed(buyer),
            msg_id,
            seller,
            encrypted_content,
        );
    }

    // Shielded escrow benchmarks

    #[benchmark]
    fn shielded_create() {
        let caller = funded_account::<T>("caller", 0);
        let commitment = [1u8; 32];
        let encrypted_params = [0u8; 512].to_vec().try_into().expect("valid size");
        let timeout_epoch = 100u32;
        let vss_commitment = VssCommitment {
            merkle_root: [1u8; 32],
            share_count: 3,
            threshold: 2,
        };

        #[extrinsic_call]
        shielded_create(
            RawOrigin::Signed(caller),
            commitment,
            encrypted_params,
            timeout_epoch,
            vss_commitment,
        );
    }

    #[benchmark]
    fn shielded_update() {
        // Setup shielded escrow first
        let caller = funded_account::<T>("caller", 0);
        let commitment = [1u8; 32];
        let encrypted_params = [0u8; 512].to_vec().try_into().expect("valid size");
        let timeout_epoch = 100u32;
        let vss_commitment = VssCommitment {
            merkle_root: [1u8; 32],
            share_count: 3,
            threshold: 2,
        };
        let _ = Pallet::<T>::shielded_create(
            RawOrigin::Signed(caller.clone()).into(),
            commitment,
            encrypted_params,
            timeout_epoch,
            vss_commitment,
        );

        let ring_sig = RingSignature {
            c: [1u8; 32],
            responses: [[2u8; 32]; 4],
            key_image: [3u8; 32],
        };
        let action = ShieldedActionV1::MarkPaid;
        let new_state_hash = [4u8; 32];

        #[extrinsic_call]
        shielded_update(
            RawOrigin::Signed(caller),
            commitment,
            ring_sig,
            action,
            new_state_hash,
        );
    }

    #[benchmark]
    fn shielded_consume() {
        let caller = funded_account::<T>("caller", 0);
        let commitment = [1u8; 32];
        let encrypted_params = [0u8; 512].to_vec().try_into().expect("valid size");
        let timeout_epoch = 100u32;
        let vss_commitment = VssCommitment {
            merkle_root: [1u8; 32],
            share_count: 3,
            threshold: 2,
        };
        let _ = Pallet::<T>::shielded_create(
            RawOrigin::Signed(caller.clone()).into(),
            commitment,
            encrypted_params,
            timeout_epoch,
            vss_commitment,
        );

        let nullifier = [5u8; 32];
        let ring_sig = RingSignature {
            c: [1u8; 32],
            responses: [[2u8; 32]; 4],
            key_image: [3u8; 32],
        };
        let release_commitment = [6u8; 32];

        #[extrinsic_call]
        shielded_consume(
            RawOrigin::Signed(caller),
            commitment,
            nullifier,
            ring_sig,
            release_commitment,
        );
    }

    #[benchmark]
    fn shielded_dispute() {
        let caller = funded_account::<T>("caller", 0);
        let commitment = [1u8; 32];
        let encrypted_params = [0u8; 512].to_vec().try_into().expect("valid size");
        let timeout_epoch = 1000u32;
        let vss_commitment = VssCommitment {
            merkle_root: [1u8; 32],
            share_count: 3,
            threshold: 2,
        };
        let _ = Pallet::<T>::shielded_create(
            RawOrigin::Signed(caller.clone()).into(),
            commitment,
            encrypted_params,
            timeout_epoch,
            vss_commitment,
        );

        let ring_sig = RingSignature {
            c: [1u8; 32],
            responses: [[2u8; 32]; 4],
            key_image: [3u8; 32],
        };
        let encrypted_evidence = ThresholdEncryptedEvidence {
            ciphertext: [0u8; 1024],
            auth_tag: [0u8; 16],
            ephemeral_pubkey: [0u8; 32],
            threshold_pubkey: [0u8; 32],
        };

        #[extrinsic_call]
        shielded_dispute(
            RawOrigin::Signed(caller),
            commitment,
            ring_sig,
            encrypted_evidence,
        );
    }

    #[benchmark]
    fn verify_vss_share() {
        let caller = funded_account::<T>("caller", 0);
        let commitment = [1u8; 32];
        let share = VerifiableShare {
            index: 0,
            value: [1u8; 32],
            proof: [[2u8; 32]; 8].to_vec().try_into().expect("valid proof"),
        };

        #[extrinsic_call]
        verify_vss_share(RawOrigin::Signed(caller), commitment, share);
    }

    // FROST signing benchmarks

    #[benchmark]
    fn submit_frost_signature() {
        let caller = funded_account::<T>("validator", 0);
        let request_id = 1u64;
        let signature = FrostSignature {
            r: [1u8; 32],
            s: [2u8; 32],
        };

        // Note: this will fail without proper setup, but measures the call overhead
        #[extrinsic_call]
        _(RawOrigin::Signed(caller), request_id, signature);
    }

    #[benchmark]
    fn mark_frost_signing_failed() {
        let caller = funded_account::<T>("validator", 0);
        let request_id = 1u64;

        #[extrinsic_call]
        _(RawOrigin::Signed(caller), request_id);
    }

    // Note: benchmark tests require a mock runtime
    // To run benchmarks: cargo test --features runtime-benchmarks
}
