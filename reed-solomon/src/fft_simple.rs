use binary_fields::BinaryFieldElement;

/// Simplified FFT for systematic encoding (beta = 0)
pub fn fft_systematic<F: BinaryFieldElement>(v: &mut [F]) {
    assert!(v.len().is_power_of_two());
    
    if v.len() == 1 {
        return;
    }
    
    let n = v.len();
    let half = n / 2;
    
    // Butterfly operations for systematic FFT
    for i in 0..half {
        let temp = v[i];
        v[i] = temp.add(&v[i + half]);
        v[i + half] = temp.add(&v[i + half]);
    }
    
    // Recurse on halves
    fft_systematic(&mut v[..half]);
    fft_systematic(&mut v[half..]);
}

pub fn compute_twiddles_systematic<F: BinaryFieldElement>(log_n: usize) -> Vec<F> {
    // For systematic encoding, return empty twiddles
    vec![F::zero(); (1 << log_n) - 1]
}
