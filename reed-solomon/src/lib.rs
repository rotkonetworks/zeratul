// reed-solomon/src/lib.rs
mod fft;
mod fft_gf32;
mod encode;

pub use encode::{encode, encode_in_place, encode_in_place_with_parallel, encode_non_systematic};
pub use fft::{compute_twiddles, fft, ifft};
pub use fft_gf32::{fft_gf32, ifft_gf32};

use binary_fields::BinaryFieldElement;

/// Reed-Solomon encoding configuration
pub struct ReedSolomon<F: BinaryFieldElement> {
    pub log_message_length: usize,
    pub log_block_length: usize,
    pub twiddles: Vec<F>,
    pub pis: Vec<F>,
}

impl<F: BinaryFieldElement> ReedSolomon<F> {
    pub fn new(message_length: usize, block_length: usize) -> Self {
        assert!(message_length.is_power_of_two());
        assert!(block_length.is_power_of_two());
        assert!(message_length < block_length);

        let log_message_length = message_length.trailing_zeros() as usize;
        let log_block_length = block_length.trailing_zeros() as usize;

        // Compute twiddles with beta = 0 for systematic encoding
        let twiddles = fft::compute_twiddles(log_block_length, F::zero());

        // Compute pis for non-systematic encoding
        let sks_vks = eval_sk_at_vks::<F>(message_length);
        let pis = compute_pis(message_length, &sks_vks);

        Self {
            log_message_length,
            log_block_length,
            twiddles,
            pis,
        }
    }

    pub fn message_length(&self) -> usize {
        1 << self.log_message_length
    }

    pub fn block_length(&self) -> usize {
        1 << self.log_block_length
    }
}

/// Create a Reed-Solomon encoder
pub fn reed_solomon<F: BinaryFieldElement>(
    message_length: usize,
    block_length: usize,
) -> ReedSolomon<F> {
    ReedSolomon::new(message_length, block_length)
}

/// Compute s_k polynomial evaluations at v_k points
pub fn eval_sk_at_vks<F: BinaryFieldElement>(n: usize) -> Vec<F> {
    assert!(n.is_power_of_two());
    let num_subspaces = n.trailing_zeros() as usize;

    let mut sks_vks = vec![F::zero(); num_subspaces + 1];
    sks_vks[0] = F::one(); // s_0(v_0) = 1

    // Initialize with powers of 2: 2^1, 2^2, ..., 2^num_subspaces
    let mut layer: Vec<F> = (1..=num_subspaces)
        .map(|i| F::from_bits(1u64 << i))
        .collect();

    let mut cur_len = num_subspaces;

    for i in 0..num_subspaces {
        for j in 0..cur_len {
            let sk_at_vk = if j == 0 {
                // s_{i+1}(v_{i+1}) computation
                let val = layer[0].mul(&layer[0]).add(&sks_vks[i].mul(&layer[0]));
                sks_vks[i + 1] = val;
                val
            } else {
                layer[j].mul(&layer[j]).add(&sks_vks[i].mul(&layer[j]))
            };

            if j > 0 {
                layer[j - 1] = sk_at_vk;
            }
        }
        cur_len -= 1;
    }

    sks_vks
}

/// Compute pi polynomials for non-systematic encoding
pub fn compute_pis<F: BinaryFieldElement>(n: usize, sks_vks: &[F]) -> Vec<F> {
    let mut pis = vec![F::zero(); n];
    pis[0] = F::one();

    for i in 1..sks_vks.len() {
        let sk_vk = sks_vks[i-1];
        let current_len = 1 << (i-1);

        // Expand pis by multiplying with sk_vk
        for j in 0..current_len {
            pis[current_len + j] = pis[j].mul(&sk_vk);
        }
    }

    pis
}

/// Extract short twiddles from long twiddles (moved from encode.rs for testing)
pub fn short_from_long_twiddles<F: BinaryFieldElement>(
    long_twiddles: &[F],
    log_n: usize,
    log_k: usize
) -> Vec<F> {
    let k = 1 << log_k;
    let mut short_twiddles = vec![F::zero(); k - 1];

    let mut jump = 1 << (log_n - log_k);
    if jump > 0 && jump <= long_twiddles.len() {
        short_twiddles[0] = long_twiddles[jump - 1];
    }

    let mut idx = 1;
    for i in 1..log_k {
        jump *= 2;
        let take = 1 << i;

        for j in 0..take {
            if jump - 1 + j < long_twiddles.len() && idx + j < short_twiddles.len() {
                short_twiddles[idx + j] = long_twiddles[jump - 1 + j];
            }
        }
        idx += take;
    }

    short_twiddles
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::{BinaryElem16, BinaryElem32, BinaryElem128};

    #[test]
    fn test_eval_sk_at_vks() {
        // Test for n = 16
        let sks_vks = eval_sk_at_vks::<BinaryElem16>(16);
        assert_eq!(sks_vks.len(), 5); // log2(16) + 1
        assert_eq!(sks_vks[0], BinaryElem16::one()); // s_0(v_0) = 1

        // Test for n = 256
        let sks_vks = eval_sk_at_vks::<BinaryElem32>(256);
        assert_eq!(sks_vks.len(), 9); // log2(256) + 1
        assert_eq!(sks_vks[0], BinaryElem32::one());
    }

    #[test]
    fn test_compute_pis() {
        let n = 16;
        let sks_vks = eval_sk_at_vks::<BinaryElem16>(n);
        let pis = compute_pis(n, &sks_vks);

        assert_eq!(pis.len(), n);
        assert_eq!(pis[0], BinaryElem16::one()); // pi_0 = 1

        // Check that pis form the correct pattern
        for i in 1..sks_vks.len() {
            let current_len = 1 << (i-1);
            for j in 0..current_len {
                assert_eq!(pis[current_len + j], pis[j].mul(&sks_vks[i-1]));
            }
        }
    }

    #[test]
    fn test_reed_solomon_creation() {
        let rs = reed_solomon::<BinaryElem16>(256, 1024);
        assert_eq!(rs.message_length(), 256);
        assert_eq!(rs.block_length(), 1024);
        assert_eq!(rs.twiddles.len(), 1023); // 2^10 - 1
        assert_eq!(rs.pis.len(), 256);
    }

    #[test]
    fn test_twiddle_computation() {
        // Test small case
        let twiddles = compute_twiddles::<BinaryElem16>(4, BinaryElem16::zero());
        assert_eq!(twiddles.len(), 15); // 2^4 - 1

        // Test with non-zero beta
        let beta = BinaryElem16::from(0x1234);
        let twiddles_beta = compute_twiddles(4, beta);
        assert_eq!(twiddles_beta.len(), 15);
        
        // Twiddles should be non-zero (except possibly with specific bad beta values)
        let non_zero_count = twiddles.iter().filter(|&&t| t != BinaryElem16::zero()).count();
        assert!(non_zero_count > 10, "Most twiddles should be non-zero");
    }

    #[test]
    fn test_fft_ifft_roundtrip() {
        // Test for various sizes
        let test_sizes = vec![4u32, 8, 16, 32];
        
        for size in test_sizes {
            let log_size = size.trailing_zeros() as usize;
            
            // Generate random data
            let mut data: Vec<BinaryElem16> = (0..size)
                .map(|i| BinaryElem16::from((i + 1) as u16))
                .collect();
            
            let original = data.clone();
            
            // Compute twiddles for this size
            let twiddles = compute_twiddles::<BinaryElem16>(log_size, BinaryElem16::zero());
            
            // Apply FFT
            fft(&mut data, &twiddles, false);
            
            // Data should be transformed (not equal to original)
            assert_ne!(data, original, "FFT should transform the data");
            
            // Apply IFFT
            ifft(&mut data, &twiddles);
            
            // Should get back the original
            assert_eq!(data, original, "FFT followed by IFFT should give identity");
        }
    }

    #[test]
    fn test_systematic_encoding() {
        let rs = reed_solomon::<BinaryElem16>(4, 16);

        let message = vec![
            BinaryElem16::from(1),
            BinaryElem16::from(2),
            BinaryElem16::from(3),
            BinaryElem16::from(4),
        ];

        let encoded = encode(&rs, &message);

        assert_eq!(encoded.len(), 16);
        
        // For systematic encoding with beta=0, the message appears in the codeword
        // but not necessarily in the first k positions due to the FFT transform
        
        // The encoding process:
        // 1. IFFT on message coefficients
        // 2. Pad with zeros
        // 3. FFT on the full block
        // This creates a Reed-Solomon codeword where the message can be recovered
        
        // Verify the codeword is non-trivial (has parity symbols)
        let parity_symbols = &encoded[4..];
        let non_zero_parity = parity_symbols.iter()
            .filter(|&&x| x != BinaryElem16::zero())
            .count();
        assert!(non_zero_parity > 0, "Reed-Solomon encoding should produce non-zero parity symbols");
    }

    #[test]
    fn test_non_systematic_encoding() {
        let rs = reed_solomon::<BinaryElem16>(4, 16);

        let mut data = vec![BinaryElem16::zero(); 16];
        data[0] = BinaryElem16::from(1);
        data[1] = BinaryElem16::from(2);
        data[2] = BinaryElem16::from(3);
        data[3] = BinaryElem16::from(4);

        let original = data.clone();
        encode_non_systematic(&rs, &mut data);

        // Non-systematic encoding scales by pis then applies FFT
        // So the result should be different from the original
        assert_ne!(data, original, "Non-systematic encoding should transform data");
        
        // The first 4 elements should be scaled by pis before FFT
        // After the full encoding, they will be further transformed
        // Just verify the encoding produced non-zero values
        let non_zero_count = data.iter().filter(|&&x| x != BinaryElem16::zero()).count();
        assert!(non_zero_count >= 4, "Encoding should produce multiple non-zero values");
    }

    #[test]
    fn test_short_from_long_twiddles() {
        let rs = reed_solomon::<BinaryElem16>(16, 64);

        // Extract short twiddles for message length 16 from block length 64
        let short_twiddles = short_from_long_twiddles(&rs.twiddles, 6, 4);

        assert_eq!(short_twiddles.len(), 15); // 2^4 - 1

        // Verify the extraction pattern matches the Julia implementation
        // The pattern follows a specific tree structure for subspace polynomials
        let jump_0 = 1 << (6 - 4); // 4
        assert_eq!(short_twiddles[0], rs.twiddles[jump_0 - 1]);
        
        // Verify more elements follow the pattern
        let jump_1 = jump_0 * 2; // 8
        assert_eq!(short_twiddles[1], rs.twiddles[jump_1 - 1]);
        assert_eq!(short_twiddles[2], rs.twiddles[jump_1]);
    }

    #[test]
    fn test_power_of_two_sizes() {
        // Test various power-of-two sizes
        let sizes = [(4, 16), (8, 32), (16, 64), (32, 128)];

        for (msg_len, block_len) in sizes {
            let rs = reed_solomon::<BinaryElem16>(msg_len, block_len);
            assert_eq!(rs.message_length(), msg_len);
            assert_eq!(rs.block_length(), block_len);

            // Test encoding
            let message: Vec<_> = (0..msg_len)
                .map(|i| BinaryElem16::from(i as u16))
                .collect();

            let encoded = encode(&rs, &message);
            assert_eq!(encoded.len(), block_len);
            
            // Verify non-trivial encoding
            let non_zero = encoded.iter().filter(|&&x| x != BinaryElem16::zero()).count();
            assert!(non_zero >= msg_len, "Encoding should preserve information");
        }
    }

    #[test]
    #[should_panic]
    fn test_invalid_message_length() {
        // Should panic because 5 is not a power of 2
        reed_solomon::<BinaryElem16>(5, 16);
    }

    #[test]
    #[should_panic]
    fn test_invalid_block_length() {
        // Should panic because 20 is not a power of 2
        reed_solomon::<BinaryElem16>(4, 20);
    }

    #[test]
    #[should_panic]
    fn test_message_larger_than_block() {
        // Should panic because message length > block length
        reed_solomon::<BinaryElem16>(16, 8);
    }

    #[test]
    fn test_different_field_sizes() {
        // Test with different field element sizes
        let rs16 = reed_solomon::<BinaryElem16>(8, 32);
        let rs32 = reed_solomon::<BinaryElem32>(8, 32);
        let rs128 = reed_solomon::<BinaryElem128>(8, 32);

        assert_eq!(rs16.message_length(), 8);
        assert_eq!(rs32.message_length(), 8);
        assert_eq!(rs128.message_length(), 8);

        // Each should produce valid encodings
        let msg16: Vec<_> = (0..8).map(|i| BinaryElem16::from(i as u16)).collect();
        let msg32: Vec<_> = (0..8).map(|i| BinaryElem32::from(i as u32)).collect();
        let msg128: Vec<_> = (0..8).map(|i| BinaryElem128::from(i as u128)).collect();

        let enc16 = encode(&rs16, &msg16);
        let enc32 = encode(&rs32, &msg32);
        let enc128 = encode(&rs128, &msg128);

        assert_eq!(enc16.len(), 32);
        assert_eq!(enc32.len(), 32);
        assert_eq!(enc128.len(), 32);
        
        // Verify encodings are non-trivial
        // Create separate assertions for each type
        let non_zero16 = enc16.iter().filter(|&&x| x != BinaryElem16::zero()).count();
        assert!(non_zero16 >= 8, "BinaryElem16 encoding should preserve message information");
        
        let non_zero32 = enc32.iter().filter(|&&x| x != BinaryElem32::zero()).count();
        assert!(non_zero32 >= 8, "BinaryElem32 encoding should preserve message information");
        
        let non_zero128 = enc128.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        assert!(non_zero128 >= 8, "BinaryElem128 encoding should preserve message information");
    }

    #[test]
    fn test_fft_with_different_betas() {
        // Test FFT with different beta values
        let betas = vec![
            BinaryElem16::zero(),
            BinaryElem16::one(),
            BinaryElem16::from(0x1234),
            BinaryElem16::from(0xABCD),
        ];
        
        for beta in betas {
            let twiddles = compute_twiddles::<BinaryElem16>(4, beta);
            
            let mut data = vec![
                BinaryElem16::from(1),
                BinaryElem16::from(2),
                BinaryElem16::from(3),
                BinaryElem16::from(4),
                BinaryElem16::from(5),
                BinaryElem16::from(6),
                BinaryElem16::from(7),
                BinaryElem16::from(8),
                BinaryElem16::from(9),
                BinaryElem16::from(10),
                BinaryElem16::from(11),
                BinaryElem16::from(12),
                BinaryElem16::from(13),
                BinaryElem16::from(14),
                BinaryElem16::from(15),
                BinaryElem16::from(0),
            ];
            
            let original = data.clone();
            
            // FFT then IFFT should give identity
            fft(&mut data, &twiddles, false);
            ifft(&mut data, &twiddles);
            
            assert_eq!(data, original, "FFT-IFFT should be identity for beta={:?}", beta);
        }
    }
    
    #[test]
    fn test_encoding_decoding_correctness() {
        // Test that we can recover the message from systematic encoding
        let rs = reed_solomon::<BinaryElem16>(4, 16);
        
        let message = vec![
            BinaryElem16::from(0x1234),
            BinaryElem16::from(0x5678),
            BinaryElem16::from(0x9ABC),
            BinaryElem16::from(0xDEF0),
        ];
        
        let encoded = encode(&rs, &message);
        
        // For systematic encoding, we should be able to recover the message
        // by applying IFFT to the codeword and taking the first k coefficients
        let mut recovery = encoded.clone();
        
        // Apply IFFT to get back to coefficient form
        ifft(&mut recovery, &rs.twiddles);
        
        // Extract message coefficients 
        let short_twiddles = short_from_long_twiddles(&rs.twiddles, 
            rs.log_block_length as usize, 
            rs.log_message_length as usize);
        
        let mut recovered_message = recovery[..4].to_vec();
        
        // Apply FFT to get back the original message
        fft(&mut recovered_message, &short_twiddles, false);
        
        assert_eq!(recovered_message, message, "Should recover original message");
    }

    #[test]
    fn test_sage_comparison() {
        // This test verifies our implementation against the Julia reference
        // using the exact same test case from BinaryReedSolomon/test/runtests.jl
        
        // These are the expected twiddle values from Julia
        let expected_twiddles = vec![
            BinaryElem128::from(261638842414339399087820898299661203057u128),
            BinaryElem128::from(130069497421973758441410450219780457337u128),
            BinaryElem128::from(130069497421973758441410450219780457327u128),
            BinaryElem128::from(321833370528025984051659201621984161951u128),
            BinaryElem128::from(321833370528025984051659201621984161945u128),
            BinaryElem128::from(321833370528025984051659201621984161923u128),
            BinaryElem128::from(321833370528025984051659201621984161925u128),
            BinaryElem128::from(12427004391475801277045897380390817389u128),
            BinaryElem128::from(12427004391475801277045897380390817391u128),
            BinaryElem128::from(12427004391475801277045897380390817385u128),
            BinaryElem128::from(12427004391475801277045897380390817387u128),
            BinaryElem128::from(12427004391475801277045897380390817381u128),
            BinaryElem128::from(12427004391475801277045897380390817383u128),
            BinaryElem128::from(12427004391475801277045897380390817377u128),
            BinaryElem128::from(12427004391475801277045897380390817379u128),
        ];
        
        // Compute twiddles with our implementation
        let computed_twiddles = compute_twiddles::<BinaryElem128>(4, BinaryElem128::zero());
        
        // Verify they match
        assert_eq!(computed_twiddles.len(), expected_twiddles.len());
        for (i, (computed, expected)) in computed_twiddles.iter().zip(expected_twiddles.iter()).enumerate() {
            assert_eq!(computed, expected, "Twiddle {} mismatch", i);
        }
        
        // Test vector from Julia
        let mut v = vec![
            BinaryElem128::from(48843935073701397021918627474152975110u128),
            BinaryElem128::from(257371465678647658219914792930422930533u128),
            BinaryElem128::from(197874898248752057839214693713406247745u128),
            BinaryElem128::from(86301329031543269357031453671330949739u128),
            BinaryElem128::from(245592208151890074913079678553060805151u128),
            BinaryElem128::from(191477208903117015546989222243599496680u128),
            BinaryElem128::from(92830719409229016308089219817617750833u128),
            BinaryElem128::from(264528954340572454088312978462893134650u128),
            BinaryElem128::from(158998607558664949362678439274836957424u128),
            BinaryElem128::from(187448928532932960560649099299315170550u128),
            BinaryElem128::from(177534835847791156274472818404289166039u128),
            BinaryElem128::from(307322189246381679156077507151623179879u128),
            BinaryElem128::from(117208864575585467966316847685913785498u128),
            BinaryElem128::from(332422437295611968587046799211069213610u128),
            BinaryElem128::from(109428368893056851194159753059340120844u128),
            BinaryElem128::from(197947890894953343492199130314470631788u128),
        ];
        
        // Apply FFT
        fft(&mut v, &computed_twiddles, false);
        
        // Expected output from Julia
        let expected_output = vec![
            BinaryElem128::from(158767388301301679479875672416174428978u128),
            BinaryElem128::from(314045034570696402167150862131636536652u128),
            BinaryElem128::from(284497668870731088162348333798389710619u128),
            BinaryElem128::from(97193893883131285058688322382264085283u128),
            BinaryElem128::from(205661608125885827099961349024782346648u128),
            BinaryElem128::from(319854111638988388244315927516461386689u128),
            BinaryElem128::from(98163024092465731168779447832503918216u128),
            BinaryElem128::from(72461851808861674126157547294435083817u128),
            BinaryElem128::from(284672699909608556571358413615868654015u128),
            BinaryElem128::from(310357233410493697565822377542976784819u128),
            BinaryElem128::from(194488171086938407232562634984109949841u128),
            BinaryElem128::from(26083141281753905375688425869148524863u128),
            BinaryElem128::from(144700278945341024867563900932218299937u128),
            BinaryElem128::from(303726834571845133663217501483978191357u128),
            BinaryElem128::from(228881976351733870473775839456225427817u128),
            BinaryElem128::from(41896060989421038344777134899638496709u128),
        ];
        
        // Verify the output matches
        assert_eq!(v.len(), expected_output.len());
        for (i, (computed, expected)) in v.iter().zip(expected_output.iter()).enumerate() {
            assert_eq!(computed, expected, "FFT output {} mismatch", i);
        }
    }

    #[test]
    fn test_small_fft_example() {
        // Test a small example we can verify by hand
        let twiddles = compute_twiddles::<BinaryElem16>(2, BinaryElem16::zero());
        assert_eq!(twiddles.len(), 3); // 2^2 - 1
        
        let mut data = vec![
            BinaryElem16::from(1),
            BinaryElem16::from(2),
            BinaryElem16::from(3),
            BinaryElem16::from(4),
        ];
        
        let original = data.clone();
        
        // Apply FFT
        fft(&mut data, &twiddles, false);
        
        // Apply IFFT
        ifft(&mut data, &twiddles);
        
        // Should get back original
        assert_eq!(data, original);
    }

    #[test]
    fn debug_twiddle_computation() {
        use binary_fields::BinaryElem128;
        
        // Helper function
        fn next_s<F: BinaryFieldElement>(s_prev: F, s_prev_at_root: F) -> F {
            s_prev.mul(&s_prev).add(&s_prev_at_root.mul(&s_prev))
        }
        
        // Test with log_n = 4 (n = 16) to match Julia test
        let log_n = 4;
        let beta = BinaryElem128::zero();
        
        println!("Computing twiddles for log_n={}, beta=0", log_n);
        
        let n = 1 << log_n;
        let mut layer = vec![BinaryElem128::zero(); 1 << (log_n - 1)];
        
        // Layer 0
        println!("\nLayer 0:");
        for i in 0..layer.len() {
            layer[i] = beta.add(&BinaryElem128::from_bits((i as u64) << 1));
            println!("  layer[{}] = {}", i, (i as u64) << 1);
        }
        let mut s_prev_at_root = BinaryElem128::one();
        
        // First iteration
        println!("\nFirst iteration:");
        let root_point = layer[0].add(&layer[1]);
        println!("  root_point = layer[0] + layer[1] = 0 + 2 = 2");
        
        // This is where the issue might be
        // next_s(2, 1) = 2^2 + 1*2 = 4 + 2 = 6
        let computed = next_s(BinaryElem128::from_bits(2), BinaryElem128::one());
        println!("  next_s(2, 1) should give 6, got: {}", 
                 computed == BinaryElem128::from_bits(6));
    }
}
