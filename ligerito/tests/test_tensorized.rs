use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};

// Copy of tensorized dot product for testing
fn tensorized_dot_product<T, U>(row: &[T], challenges: &[U]) -> U
where
    T: BinaryFieldElement,
    U: BinaryFieldElement + From<T>,
{
    let k = challenges.len();
    if k == 0 {
        return if row.len() == 1 {
            U::from(row[0])
        } else {
            U::zero()
        };
    }

    assert_eq!(row.len(), 1 << k, "Row length must be 2^k");

    // Convert row to extension field
    let mut current: Vec<U> = row.iter().map(|&x| U::from(x)).collect();

    // Fold dimension by dimension
    for &r in challenges.iter() {
        let half = current.len() / 2;
        let one_minus_r = U::one().add(&r); // Binary field: 1-r = 1+r

        for i in 0..half {
            // Contract using Lagrange basis structure: (1-r)*left + r*right
            current[i] = current[2*i].mul(&one_minus_r)
                        .add(&current[2*i+1].mul(&r));
        }
        current.truncate(half);
    }

    current[0]
}

fn evaluate_lagrange_basis<F: BinaryFieldElement>(rs: &[F]) -> Vec<F> {
    if rs.is_empty() {
        return vec![F::one()];
    }

    let one = F::one();
    let mut current_layer = vec![one.add(&rs[0]), rs[0]];
    let mut len = 2;

    for i in 1..rs.len() {
        let mut next_layer = Vec::with_capacity(2 * len);
        let ri_plus_one = one.add(&rs[i]);

        for j in 0..len {
            next_layer.push(current_layer[j].mul(&ri_plus_one));
            next_layer.push(current_layer[j].mul(&rs[i]));
        }

        current_layer = next_layer;
        len *= 2;
    }

    current_layer
}

#[test]
fn test_tensorized_matches_naive() {
    // Test with k=3 (8 elements)
    let row: Vec<BinaryElem32> = (0..8)
        .map(|i| BinaryElem32::from_bits(i as u64))
        .collect();

    let challenges: Vec<BinaryElem128> = vec![
        BinaryElem128::from_bits(3),
        BinaryElem128::from_bits(7),
        BinaryElem128::from_bits(11),
    ];

    // Naive method
    let basis = evaluate_lagrange_basis(&challenges);
    let naive_result: BinaryElem128 = row.iter()
        .zip(basis.iter())
        .fold(BinaryElem128::zero(), |acc, (&r, &b)| {
            acc.add(&BinaryElem128::from(r).mul(&b))
        });

    // Tensorized method
    let tensorized_result = tensorized_dot_product(&row, &challenges);

    println!("Naive:      {:?}", naive_result);
    println!("Tensorized: {:?}", tensorized_result);

    assert_eq!(naive_result, tensorized_result, "Tensorized and naive methods should match");
}

#[test]
fn test_lagrange_basis_structure() {
    let challenges: Vec<BinaryElem128> = vec![
        BinaryElem128::from_bits(3),
        BinaryElem128::from_bits(7),
    ];

    let basis = evaluate_lagrange_basis(&challenges);

    println!("Basis for k=2:");
    for (i, &b) in basis.iter().enumerate() {
        println!("  basis[{}] = {:?}", i, b);
    }

    // Check structure: basis should be [(1+r0)(1+r1), (1+r0)r1, r0(1+r1), r0*r1]
    let r0 = challenges[0];
    let r1 = challenges[1];
    let one = BinaryElem128::one();

    let expected = vec![
        one.add(&r0).mul(&one.add(&r1)),
        one.add(&r0).mul(&r1),
        r0.mul(&one.add(&r1)),
        r0.mul(&r1),
    ];

    for (i, (&b, &e)) in basis.iter().zip(expected.iter()).enumerate() {
        println!("  basis[{}] = {:?}, expected = {:?}, match = {}", i, b, e, b == e);
        assert_eq!(b, e, "Basis element {} mismatch", i);
    }
}
