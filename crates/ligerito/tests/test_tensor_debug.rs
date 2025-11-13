use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};

fn tensorized_dot_product_debug<T, U>(row: &[T], challenges: &[U]) -> U
where
    T: BinaryFieldElement,
    U: BinaryFieldElement + From<T>,
{
    let k = challenges.len();
    println!("\nTensorized dot product debug (REVERSED):");
    println!("  k = {}, row.len() = {}", k, row.len());

    if k == 0 {
        return if row.len() == 1 {
            U::from(row[0])
        } else {
            U::zero()
        };
    }

    let mut current: Vec<U> = row.iter().map(|&x| U::from(x)).collect();
    println!("  Initial: {:?}", current.iter().map(|x| format!("{:?}", x)).collect::<Vec<_>>());

    // REVERSED: iterate from LAST challenge to FIRST
    for (dim, &r) in challenges.iter().enumerate().rev() {
        let half = current.len() / 2;
        let one_minus_r = U::one().add(&r);

        println!("  Dimension {} (r={:?}, 1+r={:?}):", dim, r, one_minus_r);

        for i in 0..half {
            let left = current[2*i];
            let right = current[2*i+1];
            current[i] = left.mul(&one_minus_r).add(&right.mul(&r));
            println!("    new[{}] = (1+r)*{:?} + r*{:?} = {:?}",
                     i, left, right, current[i]);
        }
        current.truncate(half);
    }

    println!("  Final result: {:?}", current[0]);
    current[0]
}

#[test]
fn test_simple_case() {
    // Test k=2 with simple row
    let row: Vec<BinaryElem32> = vec![
        BinaryElem32::from_bits(1),  // row[0]
        BinaryElem32::from_bits(2),  // row[1]
        BinaryElem32::from_bits(4),  // row[2]
        BinaryElem32::from_bits(8),  // row[3]
    ];

    let challenges: Vec<BinaryElem128> = vec![
        BinaryElem128::from_bits(3),  // r0
        BinaryElem128::from_bits(5),  // r1
    ];

    println!("\n=== Manual calculation ===");
    println!("row = [1, 2, 4, 8]");
    println!("challenges = [r0=3, r1=5]");
    println!();
    println!("Lagrange basis:");
    println!("  basis[0] = (1+r0)(1+r1) = (1+3)(1+5) = 2*4 = 8");
    println!("  basis[1] = (1+r0)*r1    = (1+3)*5   = 2*5 = 10");
    println!("  basis[2] = r0*(1+r1)    = 3*(1+5)   = 3*4 = 12");
    println!("  basis[3] = r0*r1        = 3*5       = 15");
    println!();
    println!("Dot product:");
    println!("  = 1*8 + 2*10 + 4*12 + 8*15");
    println!("  = 8 + 20 + 48 + 120");

    // Calculate manually in binary field
    let r0 = BinaryElem128::from_bits(3);
    let r1 = BinaryElem128::from_bits(5);
    let one = BinaryElem128::one();

    let basis0 = one.add(&r0).mul(&one.add(&r1));
    let basis1 = one.add(&r0).mul(&r1);
    let basis2 = r0.mul(&one.add(&r1));
    let basis3 = r0.mul(&r1);

    println!("\nActual basis values:");
    println!("  basis[0] = {:?}", basis0);
    println!("  basis[1] = {:?}", basis1);
    println!("  basis[2] = {:?}", basis2);
    println!("  basis[3] = {:?}", basis3);

    let manual_result = BinaryElem128::from(row[0]).mul(&basis0)
        .add(&BinaryElem128::from(row[1]).mul(&basis1))
        .add(&BinaryElem128::from(row[2]).mul(&basis2))
        .add(&BinaryElem128::from(row[3]).mul(&basis3));

    println!("\nManual result: {:?}", manual_result);

    let tensorized_result = tensorized_dot_product_debug(&row, &challenges);

    assert_eq!(manual_result, tensorized_result);
}
